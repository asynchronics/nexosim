use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ciborium;
use serde::{de::DeserializeOwned, Serialize};

use crate::ports::EventSource;
use crate::simulation::{
    Action, Event, EventKey, SchedulerSourceRegistry, SourceId, SourceIdErased,
};

type DeserializationError = ciborium::de::Error<std::io::Error>;

/// A registry that holds all sources and sinks meant to be accessed through
/// remote procedure calls.
#[derive(Default)]
pub(crate) struct EventSourceRegistry(HashMap<String, RegistryEntry>);

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
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                s.insert(RegistryEntry::Unregistered(Box::new(Arc::new(source))));

                Ok(())
            }
            Entry::Occupied(_) => Err(source),
        }
    }

    /// Returns a mutable reference to the specified event source if it is in
    /// the registry.
    pub(crate) fn get(&self, name: &str) -> Option<&dyn EventSourceAny> {
        match self.0.get(name) {
            Some(RegistryEntry::Registered(source)) => Some(source.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn get_source_id<T: 'static>(&self, name: &str) -> Option<SourceId<T>> {
        // Downcast_ref used as a runtime type-check.
        (self.get(name)? as &dyn Any).downcast_ref().copied()
    }

    pub(crate) fn register_scheduler(&mut self, registry: &mut SchedulerSourceRegistry) {
        for entry in self.0.values_mut() {
            if let RegistryEntry::Unregistered(source) = entry {
                *entry = source.register(registry)
            }
        }
    }
}

impl fmt::Debug for EventSourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSourceRegistry ({} sources)", self.0.len())
    }
}

enum RegistryEntry {
    Unregistered(Box<dyn UnregisteredSource>),
    Registered(Box<dyn EventSourceAny>),
}

/// A type-erased `EventSource` that operates on CBOR-encoded serialized events.
pub(crate) trait EventSourceAny: Any + Send + Sync + 'static {
    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, DeserializationError>;

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(&self, serialized_arg: &[u8])
        -> Result<(Event, EventKey), DeserializationError>;

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, DeserializationError>;

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), DeserializationError>;

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str;
}

trait UnregisteredSource: Send + Sync + 'static {
    fn register(&self, registry: &mut SchedulerSourceRegistry) -> RegistryEntry;
}

impl<T> UnregisteredSource for Arc<EventSource<T>>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn register(&self, registry: &mut SchedulerSourceRegistry) -> RegistryEntry {
        let source_id = registry.add(self.clone());
        RegistryEntry::Registered(Box::new(source_id))
    }
}

impl<T> EventSourceAny for SourceId<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg| Event::new(self, arg))
    }

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(
        &self,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), DeserializationError> {
        let key = EventKey::new();
        ciborium::from_reader(serialized_arg)
            .map(|arg| (Event::new(self, arg).with_key(key.clone()), key))
    }

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg| Event::new(self, arg).with_period(period))
    }

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), DeserializationError> {
        let key = EventKey::new();
        ciborium::from_reader(serialized_arg).map(|arg| {
            (
                Event::new(self, arg)
                    .with_period(period)
                    .with_key(key.clone()),
                key,
            )
        })
    }

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}
