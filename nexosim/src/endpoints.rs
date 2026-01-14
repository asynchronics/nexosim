//! Registry for sinks and sources.
//!
//! This module provides the [`Endpoints`] object which associates each event
//! sink, event source and query source in a simulation bench to a unique name.

use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;

mod event_sink_info_registry;
mod event_sink_registry;
mod event_source_registry;
mod query_source_registry;

use crate::model::{Message, MessageSchema};
use crate::path::Path;
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
        path: impl Into<Path>,
    ) -> Result<Box<dyn EventSinkReader<T>>, EndpointError>
    where
        T: Clone + Send + 'static,
    {
        self.event_sink_registry.take(path.into())
    }

    /// Returns the [`EventId`] corresponding to an
    /// [`EventSource`](crate::ports::EventSource)`.
    ///
    /// The [`EventId`] can be used to process or schedule events.
    pub fn get_event_source_id<T>(&self, path: impl Into<Path>) -> Result<EventId<T>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry.get_source_id(&path.into())
    }

    /// Returns an iterator over the paths of all registered event sources.
    pub fn list_event_sources(&self) -> impl Iterator<Item = &Path> {
        self.event_source_registry.list_sources()
    }

    /// Returns the event schema of the specified event source if it is in the
    /// endpoint directory.
    pub fn get_event_source_schema(
        &self,
        path: impl Into<Path>,
    ) -> Result<MessageSchema, EndpointError> {
        self.event_source_registry.get_source_schema(&path.into())
    }

    /// Returns an iterator over the paths of all registered query sources.
    pub fn list_query_sources(&self) -> impl Iterator<Item = &Path> {
        self.query_source_registry.list_sources()
    }

    /// Returns the request and reply schemas of the specified query source if
    /// it is in the endpoint directory.
    pub fn get_query_source_schema(
        &self,
        path: impl Into<Path>,
    ) -> Result<(MessageSchema, MessageSchema), EndpointError> {
        self.query_source_registry.get_source_schema(&path.into())
    }

    /// Returns the [`QueryId`] corresponding to a
    /// [`QuerySource`](crate::ports::QuerySource)`.
    ///
    /// The [`QueryId`] can be used to process or schedule queries.
    pub fn get_query_source_id<T, R>(
        &self,
        path: impl Into<Path>,
    ) -> Result<QueryId<T, R>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        self.query_source_registry.get_source_id(&path.into())
    }

    /// Returns an iterator over the paths of all registered event sinks.
    pub fn list_event_sinks(&self) -> impl Iterator<Item = &Path> {
        self.event_sink_info_registry.list_sinks()
    }

    /// Returns the event schema of the specified event sink if it is in the
    /// endpoint directory.
    pub fn get_event_sink_schema(
        &self,
        path: impl Into<Path>,
    ) -> Result<MessageSchema, EndpointError> {
        self.event_sink_info_registry.get_sink_schema(&path.into())
    }
}

/// An error returned when an operation on the endpoint directory is
/// unsuccessful.
#[derive(Debug)]
#[non_exhaustive]
pub enum EndpointError {
    /// The requested event source has not been found.
    EventSourceNotFound {
        /// Path to the event source.
        path: Path,
    },
    /// The requested query source has not been found.
    QuerySourceNotFound {
        /// Path to the query source.
        path: Path,
    },
    /// The requested event sink has not been found.
    EventSinkNotFound {
        /// Path to the event sink.
        path: Path,
    },
    /// The type of the requested event source is invalid.
    InvalidEventSourceType {
        /// Path to the event source.
        path: Path,
        /// The user-requested event type.
        found_event_type: &'static str,
        /// The actual event type.
        expected_event_type: &'static str,
    },
    /// The type of the requested query source is invalid.
    InvalidQuerySourceType {
        /// Path to the query source.
        path: Path,
        /// The user-requested request type.
        found_request_type: &'static str,
        /// The user-requested reply type.
        found_reply_type: &'static str,
        /// The actual request type.
        expected_request_type: &'static str,
        /// The actual reply type.
        expected_reply_type: &'static str,
    },
    /// The type of the requested event sink is invalid.
    InvalidEventSinkType {
        /// Path to the event sink.
        path: Path,
        /// The user-requested event type.
        found_event_type: &'static str,
        /// The actual event type.
        expected_event_type: &'static str,
    },
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EventSourceNotFound { path } => {
                write!(
                    f,
                    "event source '{path}' was not found in the endpoint directory"
                )
            }
            Self::QuerySourceNotFound { path } => {
                write!(
                    f,
                    "query source '{path}' was not found in the endpoint directory"
                )
            }
            Self::EventSinkNotFound { path } => {
                write!(
                    f,
                    "event sink '{path}' was not found in the endpoint directory"
                )
            }
            Self::InvalidEventSourceType {
                path,
                found_event_type,
                expected_event_type,
            } => {
                write!(
                    f,
                    "event type '{found_event_type}' was specified for event source '{path}' but the expected type is {expected_event_type}"
                )
            }
            Self::InvalidQuerySourceType {
                path,
                found_request_type,
                found_reply_type,
                expected_request_type,
                expected_reply_type,
            } => {
                write!(
                    f,
                    "the request-reply type pair ('{found_request_type}', '{found_reply_type}') was specified for query source '{path}' but the expected types are ('{expected_request_type}', '{expected_reply_type}')"
                )
            }
            Self::InvalidEventSinkType {
                path,
                found_event_type,
                expected_event_type,
            } => {
                write!(
                    f,
                    "event type '{found_event_type}' was specified for event sink '{path}' but the expected type is {expected_event_type}"
                )
            }
        }
    }
}

impl std::error::Error for EndpointError {}
