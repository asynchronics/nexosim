use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

use serde::Serialize;

use super::{EndpointError, Message, MessageSchema};

/// A registry that holds information about the event schemas of the sinks.
#[derive(Default)]
pub(crate) struct EventSinkInfoRegistry(HashMap<String, EventSinkInfo>);

impl EventSinkInfoRegistry {
    /// Registers the event type of a sink in the registry.
    ///
    /// If the specified name is already used by another sink, and error is
    /// returned.
    pub(crate) fn register<T>(&mut self, name: String) -> Result<(), String>
    where
        T: Serialize + Message + 'static,
    {
        self.register_any::<_>(name, T::schema)
    }

    /// Registers the event type of a sink in the registry without a schema
    /// definition.
    ///
    /// If the specified name is already used by another sink, and error is
    /// returned.
    pub(crate) fn register_raw(&mut self, name: String) -> Result<(), String> {
        self.register_any::<_>(name, String::new)
    }

    /// Registers the event type of a sink in the registry, possibly with an
    /// empty schema definition.
    ///
    /// If the specified name is already used by another sink, and error is
    /// returned.
    fn register_any<F>(&mut self, name: String, schema_gen: F) -> Result<(), String>
    where
        F: Fn() -> MessageSchema + Send + Sync + 'static,
    {
        match self.0.entry(name) {
            Entry::Vacant(s) => {
                s.insert(EventSinkInfo {
                    event_schema_gen: Box::new(schema_gen),
                });

                Ok(())
            }
            Entry::Occupied(e) => Err(e.key().clone()),
        }
    }

    /// Returns an iterator over the names of all sinks in the registry.
    pub(crate) fn list_all(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(|s| s.as_str())
    }

    /// Returns the schema of the event of the specified sink if it is in the
    /// registry.
    pub(crate) fn event_schema(&self, name: &str) -> Result<MessageSchema, EndpointError> {
        self.0
            .get(name)
            .map(|info| (info.event_schema_gen)())
            .ok_or_else(|| EndpointError::EventSinkNotFound {
                name: name.to_string(),
            })
    }
}

impl fmt::Debug for EventSinkInfoRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSinkInfoRegistry ({} sinks)", self.0.len())
    }
}

/// A type-erased `EventSinkReader`.
pub(crate) struct EventSinkInfo {
    /// Function returning the schema of the events provided by the sink.
    ///
    /// If the sink was added via `add_raw` method, the function is expected to
    /// return an empty schema string.
    event_schema_gen: Box<dyn Fn() -> MessageSchema + Send + Sync + 'static>,
}
