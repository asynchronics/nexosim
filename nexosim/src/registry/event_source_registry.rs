use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ciborium;
use serde::{de::DeserializeOwned, Serialize};

use crate::ports::EventSource;
use crate::simulation::{Action, Event, EventKey, SchedulerSourceRegistry, SourceId};

use super::RegistryError;

/// A registry that holds all sources and sinks meant to be accessed through
/// remote procedure calls.
#[derive(Default)]
pub(crate) struct EventSourceRegistry(HashMap<String, Box<dyn EventSourceAny>>);

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
                s.insert(Box::new(Arc::new(source)));

                Ok(())
            }
            Entry::Occupied(_) => Err(source),
        }
    }

    /// Returns a reference to the specified event source if it is in
    /// the registry.
    pub(crate) fn get(&self, name: &str) -> Option<&dyn EventSourceAny> {
        self.0.get(name).map(|s| s.as_ref())
    }

    pub(crate) fn get_source_id<T>(&self, name: &str) -> Result<SourceId<T>, RegistryError>
    where
        T: Clone + Send + 'static,
    {
        // Downcast_ref used as a runtime type-check.
        (self.get(name).ok_or(RegistryError::NotFound)? as &dyn Any)
            .downcast_ref::<Arc<EventSource<T>>>()
            .ok_or(RegistryError::InvalidType(std::any::type_name::<
                Arc<EventSource<T>>,
            >()))?
            .source_id
            .get()
            .ok_or(RegistryError::Unregistered)
            .copied()
    }

    pub(crate) fn register_scheduler(&mut self, registry: &mut SchedulerSourceRegistry) {
        for entry in self.0.values_mut() {
            entry.register(registry);
        }
    }
}

impl fmt::Debug for EventSourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSourceRegistry ({} sources)", self.0.len())
    }
}

/// A type-erased `EventSource` that operates on CBOR-encoded serialized events.
pub(crate) trait EventSourceAny: Any + Send + Sync + 'static {
    fn action(&self, serialized_arg: &[u8]) -> Result<Action, RegistryError>;

    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, RegistryError>;

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(&self, serialized_arg: &[u8]) -> Result<(Event, EventKey), RegistryError>;

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, RegistryError>;

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Event, EventKey), RegistryError>;

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str;

    fn register(&self, registry: &mut SchedulerSourceRegistry);
}

impl<T> EventSourceAny for Arc<EventSource<T>>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn action(&self, serialized_arg: &[u8]) -> Result<Action, RegistryError> {
        ciborium::from_reader(serialized_arg)
            .map(|arg| EventSource::action(self, arg))
            .map_err(RegistryError::DeserializationError)
    }

    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn event(&self, serialized_arg: &[u8]) -> Result<Event, RegistryError> {
        let source_id = self.source_id.get().ok_or(RegistryError::Unregistered)?;
        ciborium::from_reader(serialized_arg)
            .map(|arg| Event::new(source_id, arg))
            .map_err(RegistryError::DeserializationError)
    }

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(&self, serialized_arg: &[u8]) -> Result<(Event, EventKey), RegistryError> {
        let source_id = self.source_id.get().ok_or(RegistryError::Unregistered)?;
        let key = EventKey::new();
        ciborium::from_reader(serialized_arg)
            .map(|arg| (Event::new(source_id, arg).with_key(key.clone()), key))
            .map_err(RegistryError::DeserializationError)
    }

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Event, RegistryError> {
        let source_id = self.source_id.get().ok_or(RegistryError::Unregistered)?;
        ciborium::from_reader(serialized_arg)
            .map(|arg| Event::new(source_id, arg).with_period(period))
            .map_err(RegistryError::DeserializationError)
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
    ) -> Result<(Event, EventKey), RegistryError> {
        let source_id = self.source_id.get().ok_or(RegistryError::Unregistered)?;
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

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn register(&self, registry: &mut SchedulerSourceRegistry) {
        // TODO handle double registration error?
        let _ = self.source_id.set(registry.add(self.clone()));
    }
}
