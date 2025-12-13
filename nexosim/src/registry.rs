//! Registry for sinks and sources.
//!
//! This module provides the `EndpointRegistry` object which associates each
//! event sink, event source and query source in a simulation bench to a unique
//! name.

use std::fmt::Debug;

mod event_sink_registry;
mod event_source_registry;
mod query_source_registry;

use serde::{de::DeserializeOwned, ser::Serialize};

use crate::model::{Message, MessageSchema};
use crate::ports::{EventSinkReader, RegisteredEventSource, RegisteredQuerySource};
use crate::simulation::{EventId, QueryId, SchedulerRegistry};

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
    /// Adds an event source to the registry.
    ///
    /// If the specified name is already in use for another event source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_event_source<T>(
        &mut self,
        source: RegisteredEventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), RegisteredEventSource<T>>
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
    pub(crate) fn add_event_source_raw<T>(
        &mut self,
        source: RegisteredEventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), RegisteredEventSource<T>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.add_raw(source, name)
    }

    /// Adds a query source to the registry.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_query_source<T, R>(
        &mut self,
        source: RegisteredQuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), RegisteredQuerySource<T, R>>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.query_source_registry.add(source, name)
    }

    /// Adds a query source to the registry without requiring `Message`
    /// implementations for its query and response types.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_query_source_raw<T, R>(
        &mut self,
        source: RegisteredQuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), RegisteredQuerySource<T, R>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.query_source_registry.add_raw(source, name)
    }

    /// Adds an event sink to the registry.
    ///
    /// If the specified name is already in use for another event sink, the
    /// event sink provided as argument is returned in the error.
    pub(crate) fn add_event_sink<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
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
    pub(crate) fn add_event_sink_raw<S>(
        &mut self,
        sink: S,
        name: impl Into<String>,
    ) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize,
    {
        self.event_sink_registry.add_raw(sink, name)
    }

    /// Returns an iterator over the names of the registered query sources.
    pub fn list_query_sources(&self) -> impl Iterator<Item = &String> {
        self.query_source_registry.list_sources()
    }

    /// Returns the input and output schemas of the specified query source if it
    /// is in the registry.
    pub fn get_query_source_schema(
        &self,
        name: &str,
    ) -> Result<(MessageSchema, MessageSchema), RegistryError> {
        self.query_source_registry.get_source_schema(name)
    }

    /// Returns an immutable reference to a QuerySource registered by a given
    /// name.
    pub fn get_query_source<T, R>(
        &self,
        name: &str,
    ) -> Result<&RegisteredQuerySource<T, R>, RegistryError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        self.query_source_registry.get_source(name)
    }

    pub fn get_query_source_id<T, R>(&self, name: &str) -> Result<QueryId<T, R>, RegistryError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        self.query_source_registry.get_source_id(name)
    }

    /// Returns an iterator over the names (keys) of the registered event
    /// sources.
    pub fn list_event_sources(&self) -> impl Iterator<Item = &String> {
        self.event_source_registry.list_sources()
    }

    /// Returns the schema of the specified event source if it is in the
    /// registry.
    pub fn get_event_source_schema(&self, name: &str) -> Result<MessageSchema, RegistryError> {
        self.event_source_registry.get_source_schema(name)
    }

    /// Returns an immutable reference to an EventSource registered by a given
    /// name.
    pub fn get_event_source<T>(
        &self,
        name: &str,
    ) -> Result<&RegisteredEventSource<T>, RegistryError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.get_source(name)
    }

    /// Returns a typed SourceId for an EventSource registered by a given name.
    ///
    /// SourceId can be used to schedule events on the Scheduler instance.
    pub fn get_event_source_id<T>(&self, name: &str) -> Result<EventId<T>, RegistryError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.get_source_id(name)
    }

    /// Returns an iterator over the names of all sinks in the registry.
    pub fn list_sinks(&self) -> impl Iterator<Item = &String> {
        self.event_sink_registry.list_sinks()
    }

    /// Returns the schema of the specified sink if it is in the registry.
    pub fn get_sink_schema(&self, name: &str) -> Result<MessageSchema, RegistryError> {
        self.event_sink_registry.get_sink_schema(name)
    }

    /// Returns a clone of the specified sink reader if it is in the registry.
    pub fn get_sink<E>(&self, name: &str) -> Result<E, RegistryError>
    where
        E: EventSinkReader + Send + Sync + 'static,
    {
        self.event_sink_registry.get_sink_reader(name)
    }
}

/// An error returned when an invalid endpoint registry action is taken.
#[derive(Debug)]
#[non_exhaustive]
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
    #[cfg(feature = "server")]
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
            #[cfg(feature = "server")]
            Self::DeserializationError(e) => std::fmt::Display::fmt(e, f),
        }
    }
}

impl std::error::Error for RegistryError {}
