//! Registry for sinks and sources.
//!
//! This module provides the `EndpointRegistry` object which associates each
//! event sink, event source and query source in a simulation bench to a unique
//! name.

use std::any::Any;
use std::fmt::Debug;
use std::sync::Arc;

mod event_sink_registry;
mod event_source_registry;
mod query_source_registry;

use serde::{de::DeserializeOwned, ser::Serialize};

use crate::ports::{EventSinkReader, EventSource, QuerySource};
use crate::simulation::SourceId;

pub(crate) use event_sink_registry::EventSinkRegistry;
pub(crate) use event_source_registry::EventSourceRegistry;
pub(crate) use query_source_registry::QuerySourceRegistry;

/// A registry that holds the sources and sinks of a simulation bench.
#[derive(Default, Debug)]
pub struct EndpointRegistry {
    pub(crate) event_sink_registry: EventSinkRegistry,
    pub(crate) event_source_registry: EventSourceRegistry,
    pub(crate) query_source_registry: QuerySourceRegistry,
}

impl EndpointRegistry {
    /// Creates a new, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an event source to the registry.
    ///
    /// If the specified name is already in use for another event source, the
    /// source provided as argument is returned in the error.
    pub fn add_event_source<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), EventSource<T>>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.add(source, name)
    }

    /// Adds an event source to the registry without requiring a `Message`
    /// implementation for its item type.
    ///
    /// If the specified name is already in use for another event source, the
    /// source provided as argument is returned in the error.
    pub fn add_event_source_raw<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), EventSource<T>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.add_raw(source, name)
    }

    /// Adds a query source to the registry.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub fn add_query_source<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), QuerySource<T, R>>
    where
        T: Message + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.query_source_registry.add(source, name)
    }

    /// Adds a query source to the registry without requiring `Message`
    /// implementations for its query and response types.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub fn add_query_source_raw<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), QuerySource<T, R>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.query_source_registry.add_raw(source, name)
    }

    /// Adds an event sink to the registry.
    ///
    /// If the specified name is already in use for another event sink, the
    /// event sink provided as argument is returned in the error.
    pub fn add_event_sink<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Message + Serialize,
    {
        self.event_sink_registry.add(sink, name)
    }

    /// Adds an event sink to the registry without requiring a `Message`
    /// implementation for its item type.
    ///
    /// If the specified name is already in use for another event sink, the
    /// event sink provided as argument is returned in the error.
    pub fn add_event_sink_raw<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize,
    {
        self.event_sink_registry.add_raw(sink, name)
    }

    /// Returns a typed SourceId for an EventSource registered by a given name.
    ///
    /// SourceId can be used to schedule events on the Scheduler instance.
    pub fn get_source_id<T>(&self, name: &str) -> Result<SourceId<T>, RegistryError>
    where
        T: Clone + Send + 'static,
    {
        self.event_source_registry.get_source_id(name)
    }

    /// Returns an immutable reference to a QuerySource registered by a given
    /// name.
    pub fn get_query_source<T, R>(&self, name: &str) -> Result<&QuerySource<T, R>, RegistryError>
    where
        T: Clone + Send + 'static,
        R: Send + 'static,
    {
        (self
            .query_source_registry
            .get(name)
            .ok_or(RegistryError::NotFound)? as &dyn Any)
            .downcast_ref()
            .ok_or(RegistryError::InvalidType(std::any::type_name::<
                QuerySource<T, R>,
            >()))
    }

    /// Returns an immutable reference to an EventSource registered by a given
    /// name.
    pub fn get_event_source<T>(&self, name: &str) -> Result<&EventSource<T>, RegistryError>
    where
        T: Clone + Send + 'static,
    {
        Ok((self
            .event_source_registry
            .get(name)
            .ok_or(RegistryError::NotFound)? as &dyn Any)
            .downcast_ref::<Arc<EventSource<T>>>()
            .ok_or(RegistryError::InvalidType(std::any::type_name::<
                Arc<EventSource<T>>,
            >()))?
            .as_ref())
}

/// An error returned when an invalid endpoint registry action is taken.
#[derive(Debug)]
pub enum RegistryError {
    /// The requested source has not been found.
    SourceNotFound(String),
    /// The requested sink has not been found.
    SinkNotFound(String),
    /// The requested source has not been properly registered.
    Unregistered,
    /// The requested source has an invalid type.
    InvalidType(&'static str),
    /// An argument for the source cannot be deserialized.
    DeserializationError(ciborium::de::Error<std::io::Error>),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::SourceNotFound(name) => {
                write!(f, "source not found in the registry: {name}")
            }
            RegistryError::SinkNotFound(name) => {
                write!(f, "sink not found in the registry: {name}")
            }
            Self::Unregistered => {
                f.write_str("the requested resource has not been properly registered")
            }
            Self::InvalidType(type_name) => {
                write!(f, "the requested resource cannot be cast to {type_name}")
            }
            Self::DeserializationError(e) => std::fmt::Display::fmt(e, f),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Type alias for the generated schema type.
pub type MessageSchema = String;

/// An optional helper trait for event and query input / output arguments.
/// Enables json schema generation to precisely describe the types of exchanged
/// data.
pub trait Message {
    /// Returns a schema defining message type.
    fn schema() -> MessageSchema;
}
impl<T> Message for T
where
    T: crate::JsonSchema,
{
    fn schema() -> MessageSchema {
        schemars::schema_for!(T).as_value().to_string()
    }
}

