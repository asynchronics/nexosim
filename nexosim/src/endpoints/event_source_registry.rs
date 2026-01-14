#[cfg(feature = "server")]
use std::any;
use std::any::Any;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
#[cfg(feature = "server")]
use std::time::Duration;

#[cfg(feature = "server")]
use ciborium;

use serde::{Serialize, de::DeserializeOwned};

use crate::path::Path;
use crate::ports::EventSource;
use crate::simulation::{EventId, EventIdErased, SchedulerRegistry};

#[cfg(feature = "server")]
use crate::simulation::{Event, EventKey};

use super::{EndpointError, Message, MessageSchema};

#[cfg(feature = "server")]
type DeserializationError = ciborium::de::Error<std::io::Error>;

/// A registry that holds all event sources meant to be accessed through remote
/// procedure calls.
#[derive(Default)]
pub(crate) struct EventSourceRegistry(HashMap<Path, Box<dyn EventSourceEntryAny>>);

impl EventSourceRegistry {
    /// Adds an event source to the registry.
    ///
    /// If the specified path is already used by another event source, the path
    /// to the source and the event source itself are returned in the error.
    pub(crate) fn add<T>(
        &mut self,
        source: EventSource<T>,
        path: Path,
        registry: &mut SchedulerRegistry,
    ) -> Result<(), (Path, EventSource<T>)>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.add_any(source, path, T::schema, registry)
    }

    /// Adds an event source without a schema definition to the registry.
    ///
    /// If the specified path is already used by another event source, the path
    /// to the source and the event source itself are returned in the error.
    pub(crate) fn add_raw<T>(
        &mut self,
        source: EventSource<T>,
        path: Path,
        registry: &mut SchedulerRegistry,
    ) -> Result<(), (Path, EventSource<T>)>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.add_any(source, path, String::new, registry)
    }

    /// Adds an event source to the registry, possibly with an empty schema
    /// definition.
    fn add_any<T, F>(
        &mut self,
        source: EventSource<T>,
        path: Path,
        schema_gen: F,
        registry: &mut SchedulerRegistry,
    ) -> Result<(), (Path, EventSource<T>)>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        F: Fn() -> MessageSchema + Send + Sync + 'static,
    {
        match self.0.entry(path) {
            Entry::Vacant(s) => {
                let event_id = registry.add_event_source(source);
                let entry = EventSourceEntry {
                    inner: event_id,
                    schema_gen,
                };
                s.insert(Box::new(entry));
                Ok(())
            }
            Entry::Occupied(e) => Err((e.key().clone(), source)),
        }
    }

    /// Returns a reference to a type-erased event source if it is in the
    /// registry.
    pub(crate) fn get(&self, path: &Path) -> Result<&dyn EventSourceEntryAny, EndpointError> {
        self.0
            .get(path)
            .map(|s| s.as_ref())
            .ok_or_else(|| EndpointError::EventSourceNotFound { path: path.clone() })
    }

    /// Returns the [`EventId`] of the requested event source if it is in the
    /// registry.
    pub(crate) fn get_source_id<T>(&self, path: &Path) -> Result<EventId<T>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        let event_id = self.get(path)?.get_event_id();

        Ok(EventId(event_id.0, std::marker::PhantomData))
    }

    /// Returns an iterator over the paths of all registered event sources.
    pub(crate) fn list_sources(&self) -> impl Iterator<Item = &Path> {
        self.0.keys()
    }

    /// Returns the schema of the specified event source if it is in the
    /// registry.
    pub(crate) fn get_source_schema(&self, path: &Path) -> Result<MessageSchema, EndpointError> {
        Ok(self.get(path)?.event_schema())
    }

    /// Returns an iterator over the paths and schemas of all registered event
    /// sources.
    #[cfg(feature = "server")]
    pub(crate) fn list_schemas(&self) -> impl Iterator<Item = (&Path, MessageSchema)> {
        self.0.iter().map(|(path, src)| (path, src.event_schema()))
    }
}

impl fmt::Debug for EventSourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSourceRegistry ({} sources)", self.0.len())
    }
}

/// A type-erased `EventSource` that operates on CBOR-encoded serialized events.
pub(crate) trait EventSourceEntryAny: Any + Send + Sync + 'static {
    /// Returns a type erased deserialized event argument.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn deserialize_arg(&self, serialized_arg: &[u8]) -> Result<Box<dyn Any>, DeserializationError>;

    /// Returns an event which, when processed, is broadcast to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, DeserializationError>;

    /// Returns a cancellable event and a cancellation key; when processed, the
    /// it is broadcast to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn keyed_event(&self, serialized_arg: &[u8])
    -> Result<(Event, EventKey), DeserializationError>;

    /// Returns a periodically recurring event which, when processed,
    /// broadcast to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, DeserializationError>;

    /// Returns a cancellable, periodically recurring event and a cancellation
    /// key; when processed, it is broadcast to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), DeserializationError>;

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    #[cfg(feature = "server")]
    fn event_type_name(&self) -> &'static str;

    /// Returns the schema of the event type.
    /// If the source was added via `add_raw` method, it returns an empty
    /// schema string.
    fn event_schema(&self) -> MessageSchema;

    /// Returns ErasedEventId reference.
    fn get_event_id(&self) -> EventIdErased;
}

struct EventSourceEntry<T, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> MessageSchema,
{
    inner: EventId<T>,
    schema_gen: F,
}

impl<T, F> EventSourceEntryAny for EventSourceEntry<T, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> MessageSchema + Send + Sync + 'static,
{
    #[cfg(feature = "server")]
    fn deserialize_arg(&self, serialized_arg: &[u8]) -> Result<Box<dyn Any>, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg: T| Box::new(arg) as Box<dyn Any>)
    }
    #[cfg(feature = "server")]
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg| Event::new(&self.inner, arg))
    }
    #[cfg(feature = "server")]
    fn keyed_event(
        &self,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), DeserializationError> {
        let key = EventKey::new();
        ciborium::from_reader(serialized_arg)
            .map(|arg| (Event::new(&self.inner, arg).with_key(key.clone()), key))
    }
    #[cfg(feature = "server")]
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, DeserializationError> {
        ciborium::from_reader(serialized_arg)
            .map(|arg| Event::new(&self.inner, arg).with_period(period))
    }
    #[cfg(feature = "server")]
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), DeserializationError> {
        let key = EventKey::new();

        ciborium::from_reader(serialized_arg).map(|arg| {
            (
                Event::new(&self.inner, arg)
                    .with_period(period)
                    .with_key(key.clone()),
                key,
            )
        })
    }
    #[cfg(feature = "server")]
    fn event_type_name(&self) -> &'static str {
        any::type_name::<T>()
    }
    fn event_schema(&self) -> MessageSchema {
        (self.schema_gen)()
    }
    fn get_event_id(&self) -> EventIdErased {
        self.inner.into()
    }
}
