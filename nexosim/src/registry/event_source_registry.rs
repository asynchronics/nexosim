use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ciborium;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::ports::EventSource;
use crate::simulation::{
    ActionKey, ScheduledEvent, SchedulerEventSource, Simulation, SourceIdErased,
};

type DeserializationError = ciborium::de::Error<std::io::Error>;

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
                s.insert(Box::new(RegisteredEventSource {
                    inner: Arc::new(source),
                    source_id: None,
                }));

                Ok(())
            }
            Entry::Occupied(_) => Err(source),
        }
    }

    /// Returns a mutable reference to the specified event source if it is in
    /// the registry.
    pub(crate) fn get(&self, name: &str) -> Option<&dyn EventSourceAny> {
        self.0.get(name).map(|s| s.as_ref())
    }

    pub(crate) fn register_scheduler_events(&mut self, simulation: &mut Simulation) {
        // TODO very PoC implementation
        let mut queue = simulation.scheduler_queue.lock().unwrap();
        for entry in self.0.values_mut() {
            let source_id = queue.registry.add_erased(entry.as_scheduler_source());
            entry.set_source_id(source_id);
        }
    }
}

impl fmt::Debug for EventSourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSourceRegistry ({} sources)", self.0.len())
    }
}

/// A type-erased `EventSource` that operates on CBOR-encoded serialized events.
pub(crate) trait EventSourceAny: Send + Sync + 'static {
    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn event(&self, serialized_arg: &[u8]) -> Result<ScheduledEvent, DeserializationError>;

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(
        &self,
        serialized_arg: &[u8],
    ) -> Result<(ScheduledEvent, ActionKey), DeserializationError>;

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<ScheduledEvent, DeserializationError>;

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(ScheduledEvent, ActionKey), DeserializationError>;

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str;

    fn into_future(
        &self,
        serialized_arg: &[u8],
    ) -> Result<Box<dyn Future<Output = ()>>, DeserializationError>;

    fn as_scheduler_source(&self) -> Box<dyn SchedulerEventSource>;
    fn set_source_id(&mut self, source_id: SourceIdErased);
}

struct RegisteredEventSource<T>
where
    T: DeserializeOwned + Clone + Send + 'static,
{
    inner: Arc<EventSource<T>>,
    source_id: Option<SourceIdErased>,
}

// impl<T> EventSourceAny for Arc<EventSource<T>>
impl<T> EventSourceAny for RegisteredEventSource<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn event(&self, serialized_arg: &[u8]) -> Result<ScheduledEvent, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg: T|
            // EventSource::event(&self.inner, arg)
            ScheduledEvent::new_erased(self.source_id.unwrap(), Box::new(arg)))
    }
    fn keyed_event(
        &self,
        serialized_arg: &[u8],
    ) -> Result<(ScheduledEvent, ActionKey), DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg: T| {
            // EventSource::keyed_event(&self.inner, arg)
            let action_key = ActionKey::new();
            (
                ScheduledEvent::new_erased(self.source_id.unwrap(), Box::new(arg))
                    .with_key(action_key.clone()),
                action_key,
            )
        })
    }
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<ScheduledEvent, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg: T|
                // EventSource::periodic_event(&self.inner, period, arg)
            ScheduledEvent::new_erased(self.source_id.unwrap(), Box::new(arg)).with_period(period))
    }
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(ScheduledEvent, ActionKey), DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg: T| {
            // self.inner.keyed_periodic_event(period, arg)
            let action_key = ActionKey::new();
            (
                ScheduledEvent::new_erased(self.source_id.unwrap(), Box::new(arg))
                    .with_key(action_key.clone())
                    .with_period(period),
                action_key,
            )
        })
    }
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
    fn into_future(
        &self,
        serialized_arg: &[u8],
    ) -> Result<Box<dyn Future<Output = ()>>, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg| {
            Box::new(EventSource::into_future(&self.inner, arg)) as Box<dyn Future<Output = ()>>
        })
    }
    fn as_scheduler_source(&self) -> Box<dyn SchedulerEventSource> {
        Box::new(self.inner.clone())
    }
    fn set_source_id(&mut self, source_id: SourceIdErased) {
        self.source_id = Some(source_id);
    }
}
