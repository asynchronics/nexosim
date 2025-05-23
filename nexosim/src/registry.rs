//! Registry for sinks and sources.
//!
//! This module provides the `EndpointRegistry` object which associates each
//! event sink, event source and query source in a simulation bench to a unique
//! name.

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

    pub fn get_source_id<T: 'static>(&self, name: &str) -> Option<SourceId<T>> {
        self.event_source_registry.get_source_id(name)
    }

    // pub fn
}
