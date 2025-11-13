use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[cfg(feature = "server")]
use std::time::Duration;

#[cfg(feature = "server")]
use ciborium;

use serde::{de::DeserializeOwned, Serialize};

use crate::ports::EventSource;
use crate::simulation::{SchedulerSourceRegistry, SourceId};

#[cfg(feature = "server")]
use crate::simulation::{Action, Event, EventKey};

use super::{Message, MessageSchema, RegistryError};

/// A registry that holds all sources and sinks meant to be accessed through
/// remote procedure calls.
#[derive(Default)]
pub(crate) struct EventSourceRegistry(HashMap<String, Box<dyn EventSourceEntryAny>>);

impl EventSourceRegistry {
    /// Adds an event source to the registry.
    ///
    /// If the specified name is already in use for another event source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), EventSource<T>>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        let name = name.into();
        self.insert_entry(source, name, T::schema)
    }

    /// Adds an event source to the registry without a schema definition.
    ///
    /// If the specified name is already in use for another event source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_raw<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), EventSource<T>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.insert_entry(source, name, String::new)
    }

    fn insert_entry<T, F>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
        schema_gen: F,
    ) -> Result<(), EventSource<T>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        F: Fn() -> MessageSchema + Send + Sync + 'static,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                let entry = EventSourceEntry {
                    inner: Arc::new(source),
                    schema_gen,
                };
                s.insert(Box::new(entry));
                Ok(())
            }
            Entry::Occupied(_) => Err(source),
        }
    }

    /// Returns a reference to the specified event source if it is in
    /// the registry.
    pub(crate) fn get(&self, name: &str) -> Option<&dyn EventSourceEntryAny> {
        self.0.get(name).map(|s| s.as_ref())
    }

    /// Returns an immutable reference to an EventSource registered by a given
    /// name.
    pub(crate) fn get_source<T>(&self, name: &str) -> Result<&EventSource<T>, RegistryError>
    where
        T: Clone + Send + 'static,
    {
        // Downcast_ref used as a runtime type-check.
        self.get(name)
            .ok_or(RegistryError::SourceNotFound(name.to_string()))?
            .get_event_source()
            .downcast_ref::<EventSource<T>>()
            .ok_or(RegistryError::InvalidType(std::any::type_name::<
                &EventSource<T>,
            >()))
    }

    /// Returns a typed SourceId of the requested EventSource.
    pub(crate) fn get_source_id<T>(&self, name: &str) -> Result<SourceId<T>, RegistryError>
    where
        T: Clone + Send + 'static,
    {
        // Downcast_ref used as a runtime type-check.
        self.get(name)
            .ok_or(RegistryError::SourceNotFound(name.to_string()))?
            .get_event_source()
            .downcast_ref::<EventSource<T>>()
            .ok_or(RegistryError::InvalidType(std::any::type_name::<
                &EventSource<T>,
            >()))?
            .source_id
            .get()
            .ok_or(RegistryError::Unregistered)
            .copied()
    }

    /// Registers event sources in the scheduler's registry in order to make
    /// them schedulable.
    pub(crate) fn register_scheduler(&mut self, registry: &mut SchedulerSourceRegistry) {
        for entry in self.0.values_mut() {
            entry.register(registry);
        }
    }

    /// Returns an iterator over the names (keys) of the registered event
    /// sources.
    pub(crate) fn list_sources(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    /// Returns the schema of the specified event source if it is in the
    /// registry.
    pub(crate) fn get_source_schema(&self, name: &str) -> Result<MessageSchema, RegistryError> {
        Ok(self
            .get(name)
            .ok_or(RegistryError::SourceNotFound(name.to_string()))?
            .get_schema())
    }
}

impl fmt::Debug for EventSourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSourceRegistry ({} sources)", self.0.len())
    }
}

/// A type-erased `EventSource` that operates on CBOR-encoded serialized events.
pub(crate) trait EventSourceEntryAny: Any + Send + Sync + 'static {
    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    #[cfg(feature = "server")]
    fn action(&self, serialized_arg: &[u8]) -> Result<Action, RegistryError>;

    /// Returns an event which, when processed, is broadcast to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, RegistryError>;

    /// Returns a cancellable event and a cancellation key; when processed, the
    /// it is broadcast to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn keyed_event(&self, serialized_arg: &[u8]) -> Result<(Event, EventKey), RegistryError>;

    /// Returns a periodically recurring event which, when processed,
    /// broadcast to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, RegistryError>;

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
    ) -> Result<(Event, EventKey), RegistryError>;

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    #[cfg(feature = "server")]
    fn event_type_name(&self) -> &'static str;

    /// Register the source in the scheduler's registry to make it schedulable.
    fn register(&self, registry: &mut SchedulerSourceRegistry);

    /// Returns the schema of the event type.
    /// If the source was added via `add_raw` method, it returns an empty
    /// schema string.
    fn get_schema(&self) -> MessageSchema;

    /// Returns EventSource reference.
    fn get_event_source(&self) -> &dyn Any;
}

struct EventSourceEntry<T, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> MessageSchema,
{
    inner: Arc<EventSource<T>>,
    schema_gen: F,
}

impl<T, F> EventSourceEntryAny for EventSourceEntry<T, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> MessageSchema + Send + Sync + 'static,
{
    #[cfg(feature = "server")]
    fn action(&self, serialized_arg: &[u8]) -> Result<Action, RegistryError> {
        ciborium::from_reader(serialized_arg)
            .map(|arg| EventSource::action(&self.inner, arg))
            .map_err(RegistryError::DeserializationError)
    }

    #[cfg(feature = "server")]
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, RegistryError> {
        let source_id = self
            .inner
            .source_id
            .get()
            .ok_or(RegistryError::Unregistered)?;
        ciborium::from_reader(serialized_arg)
            .map(|arg| Event::new(source_id, arg))
            .map_err(RegistryError::DeserializationError)
    }

    #[cfg(feature = "server")]
    fn keyed_event(&self, serialized_arg: &[u8]) -> Result<(Event, EventKey), RegistryError> {
        let source_id = self
            .inner
            .source_id
            .get()
            .ok_or(RegistryError::Unregistered)?;
        let key = EventKey::new();
        ciborium::from_reader(serialized_arg)
            .map(|arg| (Event::new(source_id, arg).with_key(key.clone()), key))
            .map_err(RegistryError::DeserializationError)
    }

    #[cfg(feature = "server")]
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, RegistryError> {
        let source_id = self
            .inner
            .source_id
            .get()
            .ok_or(RegistryError::Unregistered)?;
        ciborium::from_reader(serialized_arg)
            .map(|arg| Event::new(source_id, arg).with_period(period))
            .map_err(RegistryError::DeserializationError)
    }

    #[cfg(feature = "server")]
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), RegistryError> {
        let source_id = self
            .inner
            .source_id
            .get()
            .ok_or(RegistryError::Unregistered)?;
        let key = EventKey::new();
        ciborium::from_reader(serialized_arg)
            .map(|arg| {
                (
                    Event::new(source_id, arg)
                        .with_period(period)
                        .with_key(key.clone()),
                    key,
                )
            })
            .map_err(RegistryError::DeserializationError)
    }

    #[cfg(feature = "server")]
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn get_schema(&self) -> MessageSchema {
        (self.schema_gen)()
    }

    fn register(&self, registry: &mut SchedulerSourceRegistry) {
        // TODO handle double registration error?
        let _ = self.inner.source_id.set(registry.add(self.inner.clone()));
    }

    fn get_event_source(&self) -> &dyn Any {
        &*self.inner as &dyn Any
    }
}
