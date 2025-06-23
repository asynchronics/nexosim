use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ciborium;
use schemars::schema_for;
use serde::de::DeserializeOwned;

use crate::ports::EventSource;
use crate::simulation::{Action, ActionKey};

use super::{EventSchema, RegistryError, Schema};

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
        T: Schema + DeserializeOwned + Clone + Send + 'static,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                let entry = EventSourceEntry {
                    inner: Arc::new(source),
                    schema_gen: || schema_for!(T).as_value().to_string(),
                };
                s.insert(Box::new(entry));
                Ok(())
            }
            Entry::Occupied(_) => Err(source),
        }
    }

    pub(crate) fn add_raw<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), EventSource<T>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                let entry = EventSourceEntry {
                    inner: Arc::new(source),
                    // Empty string means no schema.
                    schema_gen: || String::new(),
                };
                s.insert(Box::new(entry));
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

    pub(crate) fn list_sources(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    pub(crate) fn get_source_schema(&self, name: &str) -> Result<EventSchema, RegistryError> {
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
pub(crate) trait EventSourceAny: Send + Sync + 'static {
    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn event(&self, serialized_arg: &[u8]) -> Result<Action, DeserializationError>;

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_event(
        &self,
        serialized_arg: &[u8],
    ) -> Result<(Action, ActionKey), DeserializationError>;

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Action, DeserializationError>;

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Action, ActionKey), DeserializationError>;

    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str;

    fn get_schema(&self) -> EventSchema;
}

struct EventSourceEntry<T, F>
where
    T: DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> EventSchema,
{
    inner: Arc<EventSource<T>>,
    schema_gen: F,
}

impl<T, F> EventSourceAny for EventSourceEntry<T, F>
where
    T: DeserializeOwned + Clone + Send + 'static,
    F: Fn() -> EventSchema + Send + Sync + 'static,
{
    fn event(&self, serialized_arg: &[u8]) -> Result<Action, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg| EventSource::event(&self.inner, arg))
    }
    fn keyed_event(
        &self,
        serialized_arg: &[u8],
    ) -> Result<(Action, ActionKey), DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg| EventSource::keyed_event(&self.inner, arg))
    }
    fn periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<Action, DeserializationError> {
        ciborium::from_reader(serialized_arg)
            .map(|arg| EventSource::periodic_event(&self.inner, period, arg))
    }
    fn keyed_periodic_event(
        &self,
        period: Duration,
        serialized_arg: &[u8],
    ) -> Result<(Action, ActionKey), DeserializationError> {
        ciborium::from_reader(serialized_arg)
            .map(|arg| self.inner.keyed_periodic_event(period, arg))
    }
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
    fn get_schema(&self) -> EventSchema {
        (self.schema_gen)()
    }
}
