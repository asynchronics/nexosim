use std::any::{self, Any};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
#[cfg(feature = "server")]
use std::time::Duration;

#[cfg(feature = "server")]
use ciborium;

use serde::{de::DeserializeOwned, Serialize};

use crate::ports::RegisteredEventSource;
use crate::simulation::EventId;

#[cfg(feature = "server")]
use crate::simulation::{Action, Event, EventKey};

use super::{EndpointError, Message, MessageSchema};

#[cfg(feature = "server")]
type DeserializationError = ciborium::de::Error<std::io::Error>;

/// A registry that holds all sources and sinks meant to be accessed through
/// remote procedure calls.
#[derive(Default)]
pub(crate) struct EventSourceRegistry(HashMap<String, Box<dyn EventSourceEntryAny>>);

impl EventSourceRegistry {
    /// Adds an event source to the registry.
    ///
    /// If the specified name is already used by another event source, the name
    /// of the source and the event source itself are returned in the error.
    pub(crate) fn add<T>(
        &mut self,
        source: RegisteredEventSource<T>,
        name: String,
    ) -> Result<(), (String, RegisteredEventSource<T>)>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.add_any(source, name, T::schema)
    }

    /// Adds an event source without a schema definition to the registry.
    ///
    /// If the specified name is already used by another event source, the name
    /// of the source and the event source itself are returned in the error.
    pub(crate) fn add_raw<T>(
        &mut self,
        source: RegisteredEventSource<T>,
        name: String,
    ) -> Result<(), (String, RegisteredEventSource<T>)>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.add_any(source, name, String::new)
    }

    /// Adds an event source to the registry, possibly with an empty schema
    /// definition.
    fn add_any<T, F>(
        &mut self,
        source: RegisteredEventSource<T>,
        name: String,
        schema_gen: F,
    ) -> Result<(), (String, RegisteredEventSource<T>)>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        F: Fn() -> MessageSchema + Send + Sync + 'static,
    {
        match self.0.entry(name.clone()) {
            Entry::Vacant(s) => {
                let entry = EventSourceEntry {
                    inner: Arc::new(source),
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
    pub(crate) fn get(&self, name: &str) -> Result<&dyn EventSourceEntryAny, EndpointError> {
        self.0
            .get(name)
            .map(|s| s.as_ref())
            .ok_or_else(|| EndpointError::EventSourceNotFound {
                name: name.to_string(),
            })
    }

    /// Removes and returns an event source.
    pub(crate) fn take<T>(&mut self, name: &str) -> Result<RegisteredEventSource<T>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        // FIXME - this won't work at the moment for sure
        match self.0.entry(name.to_string()) {
            Entry::Occupied(entry) => {
                if entry
                    .get()
                    .get_event_source()
                    .is::<RegisteredEventSource<T>>()
                {
                    // We now know that the downcast will succeed and can safely unwrap.
                    let source = entry
                        .remove_entry()
                        .1
                        .into_event_source()
                        .downcast::<RegisteredEventSource<T>>()
                        .unwrap();

                    Ok(*source)
                } else {
                    Err(EndpointError::InvalidEventSourceType {
                        name: name.to_string(),
                        event_type: any::type_name::<T>(),
                    })
                }
            }
            Entry::Vacant(_) => Err(EndpointError::EventSourceNotFound {
                name: name.to_string(),
            }),
        }
    }

    /// Returns an immutable reference to an event source if it is in the
    /// registry.
    pub(crate) fn get_source<T>(
        &self,
        name: &str,
    ) -> Result<&RegisteredEventSource<T>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.get(name)?
            .get_event_source()
            .downcast_ref::<RegisteredEventSource<T>>()
            .ok_or(EndpointError::InvalidEventSourceType {
                name: name.to_string(),
                event_type: any::type_name::<T>(),
            })
    }

    /// Returns a typed SourceId of the requested EventSource.
    pub(crate) fn get_source_id<T>(&self, name: &str) -> Result<EventId<T>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        Ok(self.get_source::<T>(name)?.event_id)
    }

    /// Returns an iterator over the names (keys) of the registered event
    /// sources.
    pub(crate) fn list_sources(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(|s| s.as_str())
    }

    /// Returns the schema of the specified event source if it is in the
    /// registry.
    pub(crate) fn get_source_schema(&self, name: &str) -> Result<MessageSchema, EndpointError> {
        Ok(self.get(name)?.event_schema())
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
    fn action(&self, serialized_arg: &[u8]) -> Result<Action, DeserializationError>;

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

    /// Returns EventSource reference.
    fn get_event_source(&self) -> &dyn Any;

    /// Consumes this entry and returns a boxed [`EventSource`].
    fn into_event_source(self: Box<Self>) -> Box<dyn Any>;
}

struct EventSourceEntry<T, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> MessageSchema,
{
    inner: Arc<RegisteredEventSource<T>>,
    schema_gen: F,
}

impl<T, F> EventSourceEntryAny for EventSourceEntry<T, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> MessageSchema + Send + Sync + 'static,
{
    #[cfg(feature = "server")]
    fn action(&self, serialized_arg: &[u8]) -> Result<Action, DeserializationError> {
        ciborium::from_reader(serialized_arg)
            .map(|arg| RegisteredEventSource::action(&self.inner, arg))
    }

    #[cfg(feature = "server")]
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg| Event::new(&self.inner.event_id, arg))
    }
    #[cfg(feature = "server")]
    fn keyed_event(
        &self,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), DeserializationError> {
        let key = EventKey::new();
        ciborium::from_reader(serialized_arg).map(|arg| {
            (
                Event::new(&self.inner.event_id, arg).with_key(key.clone()),
                key,
            )
        })
    }
    #[cfg(feature = "server")]
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, DeserializationError> {
        ciborium::from_reader(serialized_arg)
            .map(|arg| Event::new(&self.inner.event_id, arg).with_period(period))
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
                Event::new(&self.inner.event_id, arg)
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
    fn get_event_source(&self) -> &dyn Any {
        &*self.inner as &dyn Any
    }
    // FIXME: this is broken for now due to the `Arc`.
    fn into_event_source(self: Box<Self>) -> Box<dyn Any> {
        Box::new(self.inner)
    }
}
