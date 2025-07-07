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
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.add(source, name)
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
        T: DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.query_source_registry.add(source, name)
    }

    /// Adds an event sink to the registry.
    ///
    /// If the specified name is already in use for another event sink, the
    /// event sink provided as argument is returned in the error.
    pub fn add_event_sink<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize,
    {
        self.event_sink_registry.add(sink, name)
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
}

/// An error returned when an invalid endpoint registry action is taken.
#[derive(Debug)]
pub enum RegistryError {
    /// The requested source has not been found.
    NotFound,
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
            Self::NotFound => f.write_str("the requested resource is not present in the registr"),
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
