use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;

#[cfg(feature = "server")]
use std::time::Duration;

#[cfg(feature = "server")]
use ciborium;

use dyn_clone::DynClone;
use serde::Serialize;

use crate::ports::EventSinkReader;

use super::{Message, MessageSchema, RegistryError};

#[cfg(feature = "server")]
type SerializationError = ciborium::ser::Error<std::io::Error>;

/// A registry that holds all sinks meant to be accessed through remote
/// procedure calls.
#[derive(Default)]
pub(crate) struct EventSinkRegistry(HashMap<String, Box<dyn EventSinkEntryAny>>);

impl EventSinkRegistry {
    /// Adds a sink to the registry.
    ///
    /// If the specified name is already in use for another sink, the sink
    /// provided as argument is returned in the error.
    pub(crate) fn add<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize + Message,
    {
        self.insert_entry(sink, name, S::Item::schema)
    }

    /// Adds a sink to the registry without a schema definition.
    ///
    /// If the specified name is already in use for another sink, the sink
    /// provided as argument is returned in the error.
    pub(crate) fn add_raw<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize,
    {
        self.insert_entry(sink, name, String::new)
    }

    fn insert_entry<S, F>(
        &mut self,
        sink: S,
        name: impl Into<String>,
        schema_gen: F,
    ) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize,
        F: Fn() -> MessageSchema + Clone + Send + Sync + 'static,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                let entry = EventSinkEntry {
                    inner: sink,
                    schema_gen,
                };
                s.insert(Box::new(entry));

                Ok(())
            }
            Entry::Occupied(_) => Err(sink),
        }
    }

    /// Returns a mutable reference to the specified sink if it is in the
    /// registry.
    #[cfg(feature = "server")]
    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut dyn EventSinkEntryAny> {
        self.0.get_mut(name).map(|s| s.as_mut())
    }

    // Returns an unmutable reference to the specified sink if it is in the
    // registry.
    // pub(crate) fn get_ref(&mut self, name: &str) -> Option<&dyn EventSinkEntryAny> {
    //     self.0.get(name).map(|s| s.as_ref())
    // }

    /// Returns a clone of the specified sink if it is in the registry.
    pub(crate) fn get(&self, name: &str) -> Option<Box<dyn EventSinkEntryAny>> {
        self.0.get(name).map(|s| dyn_clone::clone_box(&**s))
    }

    // Returns a clone of the specified sink if it is in the registry.
    // pub fn get_sink_reader<E>(&self, name: &str) -> Result<E, RegistryError>
    // where
    //     E: EventSinkReader + Send + Sync + 'static,
    // {
    //     // Downcast_ref used as a runtime type-check.
    //     Ok(self
    //         .get_ref(name)
    //         .ok_or(RegistryError::SourceNotFound(name.to_string()))?
    //         .get_event_sink_reader()
    //         .downcast_ref::<&E>()
    //         .ok_or(RegistryError::InvalidType(std::any::type_name::<&E>()))?
    //         .clone())
    // }

    /// Returns an iterator over the names of all sinks in the registry.
    pub(crate) fn list_sinks(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    /// Returns the schema of the specified sink if it is in the registry.
    pub(crate) fn get_sink_schema(&self, name: &str) -> Result<MessageSchema, RegistryError> {
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
pub(crate) trait EventSinkEntryAny: DynClone + Send + Sync + 'static {
    /// Human-readable name of the event type, as returned by
    /// `any::type_name`.
    #[cfg(feature = "server")]
    fn event_type_name(&self) -> &'static str;

    /// Starts or resumes the collection of new events.
    #[cfg(feature = "server")]
    fn open(&mut self);

    /// Pauses the collection of new events.
    #[cfg(feature = "server")]
    fn close(&mut self);

    /// Encodes and collects all events in a vector.
    #[cfg(feature = "server")]
    fn collect(&mut self) -> Result<Vec<Vec<u8>>, SerializationError>;

    /// Waits for an event and encodes it in bytes.
    #[cfg(feature = "server")]
    fn await_event(&mut self, timeout: Duration) -> Result<Vec<u8>, SerializationError>;

    /// Returns the schema of the events provided by the sink.
    /// If the sink was added via `add_raw` method, it returns an empty schema
    /// string.
    fn get_schema(&self) -> MessageSchema;

    // Returns EventSinkReader reference.
    // fn get_event_sink_reader(&self) -> &dyn Any;
}

#[derive(Clone)]
struct EventSinkEntry<E, F>
where
    E: EventSinkReader + Send + Sync + 'static,
    F: Fn() -> MessageSchema,
{
    #[allow(dead_code)]
    inner: E,
    schema_gen: F,
}

dyn_clone::clone_trait_object!(EventSinkEntryAny);

impl<E, F> EventSinkEntryAny for EventSinkEntry<E, F>
where
    E: EventSinkReader + Send + Sync + 'static,
    E::Item: Serialize,
    F: Fn() -> MessageSchema + Clone + Send + Sync + 'static,
{
    #[cfg(feature = "server")]
    fn event_type_name(&self) -> &'static str {
        std::any::type_name::<E::Item>()
    }

    #[cfg(feature = "server")]
    fn open(&mut self) {
        self.inner.open();
    }

    #[cfg(feature = "server")]
    fn close(&mut self) {
        self.inner.close();
    }

    #[cfg(feature = "server")]
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

    #[cfg(feature = "server")]
    fn await_event(&mut self, timeout: Duration) -> Result<Vec<u8>, SerializationError> {
        self.inner.set_timeout(timeout);
        self.inner.set_blocking(true);
        // Blocking call.
        let event = self.inner.next();
        let mut buffer = Vec::new();
        ciborium::into_writer(&event, &mut buffer)?;
        Ok(buffer)
    }

    fn get_schema(&self) -> MessageSchema {
        (self.schema_gen)()
    }

    // fn get_event_sink_reader(&self) -> &dyn Any {
    //     &self.inner as &dyn Any
    // }
}
