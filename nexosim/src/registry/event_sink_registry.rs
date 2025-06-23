use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use ciborium;
use dyn_clone::DynClone;
use schemars::schema_for;
use serde::Serialize;

use crate::ports::EventSinkReader;

use super::{EventSchema, RegistryError, Schema};

type SerializationError = ciborium::ser::Error<std::io::Error>;

/// A registry that holds all sinks meant to be accessed through remote
/// procedure calls.
#[derive(Default)]
pub(crate) struct EventSinkRegistry(HashMap<String, Box<dyn EventSinkReaderAny>>);

impl EventSinkRegistry {
    /// Adds a sink to the registry.
    ///
    /// If the specified name is already in use for another sink, the sink
    /// provided as argument is returned in the error.
    pub(crate) fn add<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize + Schema,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                let entry = EventSinkEntry {
                    inner: sink,
                    schema_gen: || schema_for!(S::Item).as_value().to_string(),
                };
                s.insert(Box::new(entry));

                Ok(())
            }
            Entry::Occupied(_) => Err(sink),
        }
    }

    pub(crate) fn add_raw<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                let entry = EventSinkEntry {
                    inner: sink,
                    schema_gen: || String::new(),
                };
                s.insert(Box::new(entry));

                Ok(())
            }
            Entry::Occupied(_) => Err(sink),
        }
    }

    /// Returns a mutable reference to the specified sink if it is in the
    /// registry.
    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut dyn EventSinkReaderAny> {
        self.0.get_mut(name).map(|s| s.as_mut())
    }

    /// Returns a clone of the specified sink if it is in the registry.
    pub(crate) fn get(&self, name: &str) -> Option<Box<dyn EventSinkReaderAny>> {
        self.0.get(name).map(|s| dyn_clone::clone_box(&**s))
    }

    pub(crate) fn list_sinks(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    pub(crate) fn get_sink_schema(&self, name: &str) -> Result<EventSchema, RegistryError> {
        Ok(self
            .get(name)
            .ok_or(RegistryError::SinkNotFound(name.to_string()))?
            .get_schema())
    }
}

impl fmt::Debug for EventSinkRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSinkRegistry ({} sinks)", self.0.len())
    }
}

/// A type-erased `EventSinkReader`.
pub(crate) trait EventSinkReaderAny: DynClone + Send + Sync + 'static {
    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    fn event_type_name(&self) -> &'static str;

    /// Starts or resumes the collection of new events.
    fn open(&mut self);

    /// Pauses the collection of new events.
    fn close(&mut self);

    /// Encodes and collects all events in a vector.
    fn collect(&mut self) -> Result<Vec<Vec<u8>>, SerializationError>;

    /// Waits for an event and encodes it in bytes.
    fn await_event(&mut self, timeout: Duration) -> Result<Vec<u8>, SerializationError>;

    fn get_schema(&self) -> EventSchema;
}

#[derive(Clone)]
struct EventSinkEntry<E, F>
where
    E: EventSinkReader + Send + Sync + 'static,
    F: Fn() -> EventSchema,
{
    inner: E,
    schema_gen: F,
}

dyn_clone::clone_trait_object!(EventSinkReaderAny);

impl<E, F> EventSinkReaderAny for EventSinkEntry<E, F>
where
    E: EventSinkReader + Send + Sync + 'static,
    E::Item: Serialize,
    F: Fn() -> EventSchema + Clone + Send + Sync + 'static,
{
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<E::Item>()
    }

    fn open(&mut self) {
        self.inner.open();
    }

    fn close(&mut self) {
        self.inner.close();
    }

    fn collect(&mut self) -> Result<Vec<Vec<u8>>, SerializationError> {
        self.inner.set_blocking(false);
        self.inner
            .__try_fold(Vec::new(), |mut encoded_events, event| {
                let mut buffer = Vec::new();
                ciborium::into_writer(&event, &mut buffer).map(|_| {
                    encoded_events.push(buffer);

                    encoded_events
                })
            })
    }

    fn await_event(&mut self, timeout: Duration) -> Result<Vec<u8>, SerializationError> {
        self.inner.set_timeout(timeout);
        self.inner.set_blocking(true);
        // Blocking call.
        let event = self.inner.next();
        let mut buffer = Vec::new();
        ciborium::into_writer(&event, &mut buffer)?;
        Ok(buffer)
    }

    fn get_schema(&self) -> EventSchema {
        (self.schema_gen)()
    }
}
