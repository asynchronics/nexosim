use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ciborium;
use serde::{de::DeserializeOwned, Serialize};

use crate::ports::EventSource;
use crate::simulation::{Event, EventKey, SchedulerSourceRegistry, SourceId, SourceIdErased};

type DeserializationError = ciborium::de::Error<std::io::Error>;

/// A registry that holds all sources and sinks meant to be accessed through
/// remote procedure calls.
#[derive(Default)]
pub struct EventSourceRegistry(HashMap<String, RegistryEntry>);

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
    pub fn get(&self, name: &str) -> Option<&dyn EventSourceAny> {
        // TODO generic get for non-server use
        match self.0.get(name) {
            Some(RegistryEntry::Registered(source)) => Some(source.as_ref()),
            _ => None,
        }
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
pub trait EventSourceAny: Send + Sync + 'static {
    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn event(&self, arg: Box<dyn Any + Send>) -> Event;

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(&self, arg: Box<dyn Any + Send>) -> (Event, EventKey);

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(&self, period: Duration, arg: Box<dyn Any + Send>) -> Event;

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_periodic_event(&self, period: Duration, arg: Box<dyn Any + Send>)
        -> (Event, EventKey);

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str;

    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, DeserializationError>;
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
    fn event(&self, arg: Box<dyn Any + Send>) -> Event {
        Event::new_erased(self, arg)
    }

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(&self, arg: Box<dyn Any + Send>) -> (Event, EventKey) {
        let key = EventKey::new();
        (Event::new_erased(self, arg).with_key(key.clone()), key)
    }

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(&self, period: Duration, arg: Box<dyn Any + Send>) -> Event {
        Event::new_erased(self, arg).with_period(period)
    }

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_periodic_event(
        &self,
        period: Duration,
        arg: Box<dyn Any + Send>,
    ) -> (Event, EventKey) {
        let key = EventKey::new();
        (
            Event::new_erased(self, arg)
                .with_period(period)
                .with_key(key.clone()),
            key,
        )
    }

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, DeserializationError> {
        ciborium::from_reader::<T, _>(arg).map(|arg| Box::new(arg) as Box<dyn Any + Send>)
    }
}
