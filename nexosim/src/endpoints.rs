//! Registry for sinks and sources.
//!
//! This module provides the `EndpointRegistry` object which associates each
//! event sink, event source and query source in a simulation bench to a unique
//! name.

use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;

mod event_sink_info_registry;
mod event_sink_registry;
mod event_source_registry;
mod query_source_registry;

use crate::model::{Message, MessageSchema};
use crate::ports::EventSinkReader;
use crate::simulation::{EventId, QueryId};

pub(crate) use event_sink_info_registry::EventSinkInfoRegistry;
pub(crate) use event_sink_registry::EventSinkRegistry;
#[cfg(feature = "server")]
pub(crate) use event_source_registry::EventSourceEntryAny;
pub(crate) use event_source_registry::EventSourceRegistry;
pub(crate) use query_source_registry::QuerySourceRegistry;
#[cfg(feature = "server")]
pub(crate) use query_source_registry::{QuerySourceEntryAny, ReplyReaderAny};

/// A directory of all sources and sinks of a simulation bench.
#[derive(Default, Debug)]
pub struct Endpoints {
    event_sink_registry: EventSinkRegistry,
    event_sink_info_registry: EventSinkInfoRegistry,
    event_source_registry: EventSourceRegistry,
    query_source_registry: QuerySourceRegistry,
}

impl Endpoints {
    /// Creates a new endpoint directory from its raw components.
    pub(crate) fn new(
        event_sink_registry: EventSinkRegistry,
        event_sink_info_registry: EventSinkInfoRegistry,
        event_source_registry: EventSourceRegistry,
        query_source_registry: QuerySourceRegistry,
    ) -> Self {
        Self {
            event_sink_registry,
            event_sink_info_registry,
            event_source_registry,
            query_source_registry,
        }
    }

    /// Decomposes the endpoint directory into its raw components.
    #[cfg(feature = "server")]
    pub(crate) fn into_parts(
        self,
    ) -> (
        EventSinkRegistry,
        EventSinkInfoRegistry,
        EventSourceRegistry,
        QuerySourceRegistry,
    ) {
        (
            self.event_sink_registry,
            self.event_sink_info_registry,
            self.event_source_registry,
            self.query_source_registry,
        )
    }

    /// Extracts and returns a boxed [`EventSinkReader`] trait object from the
    /// endpoint directory.
    pub fn take_event_sink<T>(
        &mut self,
        name: &str,
    ) -> Result<Box<dyn EventSinkReader<T>>, EndpointError>
    where
        T: Clone + Send + 'static,
    {
        self.event_sink_registry.take(name)
    }

    /// Returns a typed EventId for an [`EventSource`]`.
    ///
    /// SourceId can be used to schedule events on the Scheduler instance.
    pub fn get_event_source_id<T>(&self, name: &str) -> Result<EventId<T>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.get_source_id(name)
    }

    /// Returns an iterator over the names (keys) of the registered event
    /// sources.
    pub fn list_event_sources(&self) -> impl Iterator<Item = &str> {
        self.event_source_registry.list_sources()
    }

    /// Returns the schema of the specified event source if it is in the
    /// registry.
    pub fn get_event_source_schema(&self, name: &str) -> Result<MessageSchema, EndpointError> {
        self.event_source_registry.get_source_schema(name)
    }

    /// Returns an iterator over the names of the registered query sources.
    pub fn list_query_sources(&self) -> impl Iterator<Item = &str> {
        self.query_source_registry.list_sources()
    }

    /// Returns the input and output schemas of the specified query source if it
    /// is in the registry.
    pub fn get_query_source_schema(
        &self,
        name: &str,
    ) -> Result<(MessageSchema, MessageSchema), EndpointError> {
        self.query_source_registry.get_source_schema(name)
    }

    /// Returns a typed QueryId for an [`QuerySource`]`.
    ///
    /// SourceId can be used to schedule events on the Scheduler instance.
    pub fn get_query_source_id<T, R>(&self, name: &str) -> Result<QueryId<T, R>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        self.query_source_registry.get_source_id(name)
    }

    /// Returns an iterator over the names of all sinks in the registry.
    pub fn list_event_sinks(&self) -> impl Iterator<Item = &str> {
        self.event_sink_info_registry.list_all()
    }

    /// Returns the schema of the specified sink if it is in the registry.
    pub fn get_event_sink_schema(&self, name: &str) -> Result<MessageSchema, EndpointError> {
        self.event_sink_info_registry.event_schema(name)
    }
}

/// An error returned when an operation on the endpoint directory is
/// unsuccessful.
#[derive(Debug)]
#[non_exhaustive]
pub enum EndpointError {
    /// The requested event source has not been found.
    EventSourceNotFound {
        /// Name of the event source.
        name: String,
    },
    /// The requested query source has not been found.
    QuerySourceNotFound {
        /// Name of the query source.
        name: String,
    },
    /// The requested event sink has not been found.
    EventSinkNotFound {
        /// Name of the event sink.
        name: String,
    },
    /// The type of the requested event source is invalid.
    InvalidEventSourceType {
        /// Name of the event source.
        name: String,
        /// Name of the event type.
        event_type: &'static str,
    },
    /// The type of the requested query source is invalid.
    InvalidQuerySourceType {
        /// Name of the query source.
        name: String,
        /// Name of the request type.
        request_type: &'static str,
        /// Name of the reply type.
        reply_type: &'static str,
    },
    /// The type of the requested event sink is invalid.
    InvalidEventSinkType {
        /// Name of the event sink.
        name: String,
        /// Name of the event type.
        event_type: &'static str,
    },
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EventSourceNotFound { name } => {
                write!(
                    f,
                    "event source '{name}' was not found in the endpoint registry"
                )
            }
            Self::QuerySourceNotFound { name } => {
                write!(
                    f,
                    "query source '{name}' was not found in the endpoint registry"
                )
            }
            Self::EventSinkNotFound { name } => {
                write!(
                    f,
                    "event sink '{name}' was not found in the endpoint registry"
                )
            }
            Self::InvalidEventSourceType { name, event_type } => {
                write!(
                    f,
                    "event type '{event_type}' is not valid for event source '{name}'"
                )
            }
            Self::InvalidQuerySourceType {
                name,
                request_type,
                reply_type,
            } => {
                write!(
                    f,
                    "the request-reply type pair ('{request_type}', '{reply_type}') is not valid for query source '{name}'"
                )
            }
            Self::InvalidEventSinkType { name, event_type } => {
                write!(
                    f,
                    "event type '{event_type}' is not valid for event sink '{name}'"
                )
            }
        }
    }
}

impl std::error::Error for EndpointError {}
