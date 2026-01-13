use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

use serde::Serialize;

use crate::path::Path;

use super::{EndpointError, Message, MessageSchema};

/// A registry that holds information about sink event schemas.
#[derive(Default)]
pub(crate) struct EventSinkInfoRegistry(HashMap<Path, EventSinkInfo>);

impl EventSinkInfoRegistry {
    /// Registers the event type of a sink in the registry.
    ///
    /// If the specified path is already used by another sink, an error is
    /// returned.
    pub(crate) fn register<T>(&mut self, path: Path) -> Result<(), Path>
    where
        T: Serialize + Message + 'static,
    {
        self.register_any::<_>(path, T::schema)
    }

    /// Registers the event type of a sink in the registry without a schema
    /// definition.
    ///
    /// If the specified path is already used by another sink, an error is
    /// returned.
    pub(crate) fn register_raw(&mut self, path: Path) -> Result<(), Path> {
        self.register_any::<_>(path, String::new)
    }

    /// Registers the event type of a sink in the registry, possibly with an
    /// empty schema definition.
    ///
    /// If the specified path is already used by another sink, an error is
    /// returned.
    fn register_any<F>(&mut self, path: Path, schema_gen: F) -> Result<(), Path>
    where
        F: Fn() -> MessageSchema + Send + Sync + 'static,
    {
        match self.0.entry(path) {
            Entry::Vacant(s) => {
                s.insert(EventSinkInfo {
                    event_schema_gen: Box::new(schema_gen),
                });

                Ok(())
            }
            Entry::Occupied(e) => Err(e.key().clone()),
        }
    }

    /// Returns an iterator over the paths of all registered event sinks.
    pub(crate) fn list_sinks(&self) -> impl Iterator<Item = &Path> {
        self.0.keys()
    }

    /// Returns the schema of the event of the specified sink if it is in the
    /// registry.
    pub(crate) fn get_sink_schema(&self, path: &Path) -> Result<MessageSchema, EndpointError> {
        self.0
            .get(path)
            .map(|info| (info.event_schema_gen)())
            .ok_or_else(|| EndpointError::EventSinkNotFound { path: path.clone() })
    }

    /// Returns an iterator over the paths and schemas of the registered event
    /// sinks.
    #[cfg(feature = "server")]
    pub(crate) fn list_schemas(&self) -> impl Iterator<Item = (&Path, MessageSchema)> {
        self.0
            .iter()
            .map(|(path, info)| (path, (info.event_schema_gen)()))
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
