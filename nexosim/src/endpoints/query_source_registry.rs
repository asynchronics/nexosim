use std::any::{self, Any};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

#[cfg(feature = "server")]
use ciborium;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ports::QuerySource;

#[cfg(feature = "server")]
use crate::ports::ReplyReceiver;
#[cfg(feature = "server")]
use crate::simulation::Action;

use super::{EndpointError, Message, MessageSchema};

#[cfg(feature = "server")]
type SerializationError = ciborium::ser::Error<std::io::Error>;
#[cfg(feature = "server")]
type DeserializationError = ciborium::de::Error<std::io::Error>;

/// A registry that holds all sources and sinks meant to be accessed through
/// remote procedure calls.
#[derive(Default)]
pub(crate) struct QuerySourceRegistry(HashMap<String, Box<dyn QuerySourceEntryAny>>);

impl QuerySourceRegistry {
    /// Adds a query source to the registry.
    ///
    /// If the specified name is already used by another query source, the name
    /// of the query and the query source itself are returned in the error.
    pub(crate) fn add<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: String,
    ) -> Result<(), (String, QuerySource<T, R>)>
    where
        T: Message + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.add_any(source, name, || (T::schema(), R::schema()))
    }

    /// Adds a query source to the registry without a schema definition.
    ///
    /// If the specified name is already used by another query source, the name
    /// of the query and the query source itself are returned in the error.
    pub(crate) fn add_raw<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: String,
    ) -> Result<(), (String, QuerySource<T, R>)>
    where
        T: DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.add_any(source, name, || (String::new(), String::new()))
    }

    /// Adds a query source to the registry, possibly with an empty schema definition.
    fn add_any<T, R, F>(
        &mut self,
        source: QuerySource<T, R>,
        name: String,
        schema_gen: F,
    ) -> Result<(), (String, QuerySource<T, R>)>
    where
        T: DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
        F: Fn() -> (MessageSchema, MessageSchema) + Send + Sync + 'static,
    {
        match self.0.entry(name) {
            Entry::Vacant(s) => {
                let entry = QuerySourceEntry {
                    inner: source,
                    schema_gen,
                };
                s.insert(Box::new(entry));

                Ok(())
            }
            Entry::Occupied(e) => Err((e.key().clone(), source)),
        }
    }

    /// Returns a reference to the specified query source if it is in
    /// the registry.
    pub(crate) fn get(&self, name: &str) -> Result<&dyn QuerySourceEntryAny, EndpointError> {
        self.0
            .get(name)
            .map(|s| s.as_ref())
            .ok_or_else(|| EndpointError::QuerySourceNotFound {
                name: name.to_string(),
            })
    }

    /// Returns an immutable reference to a QuerySource registered by a given
    /// name.
    pub(crate) fn get_source<T, R>(&self, name: &str) -> Result<&QuerySource<T, R>, EndpointError>
    where
        T: Clone + Send + 'static,
        R: Send + 'static,
    {
        // Downcast_ref used as a runtime type-check.
        self.get(name)?
            .get_query_source()
            .downcast_ref::<QuerySource<T, R>>()
            .ok_or(EndpointError::InvalidQuerySourceType {
                name: name.to_string(),
                request_type: any::type_name::<T>(),
                reply_type: any::type_name::<R>(),
            })
    }

    /// Returns an immutable reference to a QuerySource registered by a given
    /// name.
    pub(crate) fn take<T, R>(&mut self, name: &str) -> Result<QuerySource<T, R>, EndpointError>
    where
        T: Clone + Send + 'static,
        R: Send + 'static,
    {
        match self.0.entry(name.to_string()) {
            Entry::Occupied(entry) => {
                if entry.get().get_query_source().is::<QuerySource<T, R>>() {
                    // We now know that the downcast will succeed and can safely unwrap.
                    let source = entry
                        .remove_entry()
                        .1
                        .into_query_source()
                        .downcast::<QuerySource<T, R>>()
                        .unwrap();

                    Ok(*source)
                } else {
                    Err(EndpointError::InvalidQuerySourceType {
                        name: name.to_string(),
                        request_type: any::type_name::<T>(),
                        reply_type: any::type_name::<R>(),
                    })
                }
            }
            Entry::Vacant(_) => Err(EndpointError::QuerySourceNotFound {
                name: name.to_string(),
            }),
        }
    }

    /// Returns an iterator over the names of the registered query sources.
    pub(crate) fn list_sources(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(|s| s.as_str())
    }

    /// Returns the input and output schemas of the specified query source if it
    /// is in the registry.
    pub(crate) fn get_source_schema(
        &self,
        name: &str,
    ) -> Result<(MessageSchema, MessageSchema), EndpointError> {
        Ok(self.get(name)?.query_schema())
    }
}

impl fmt::Debug for QuerySourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "QuerySourceRegistry ({} query sources)", self.0.len(),)
    }
}

/// A type-erased `QuerySource` that operates on CBOR-encoded serialized queries
/// and returns CBOR-encoded replies.
pub(crate) trait QuerySourceEntryAny: Any + Send + Sync + 'static {
    /// Returns an action which, when processed, broadcasts a query to all
    /// connected replier ports.
    ///
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn query(
        &self,
        arg: &[u8],
    ) -> Result<(Action, Box<dyn ReplyReceiverAny>), DeserializationError>;

    /// Human-readable name of the request type, as returned by
    /// `any::type_name`.
    #[cfg(feature = "server")]
    fn request_type_name(&self) -> &'static str;

    /// Human-readable name of the reply type, as returned by
    /// `any::type_name`.
    #[cfg(feature = "server")]
    fn reply_type_name(&self) -> &'static str;

    /// Returns the input and output schemas of the query source.
    ///
    /// The first element of the tuple being the input schema, the second one
    /// the output schema.
    /// If the query was added via `add_raw` method, it returns an empty schema
    /// strings.
    fn query_schema(&self) -> (MessageSchema, MessageSchema);

    /// Returns QuerySource reference.
    fn get_query_source(&self) -> &dyn Any;

    /// Consumes this entry and returns a boxed [`QuerySource`].
    fn into_query_source(self: Box<Self>) -> Box<dyn Any>;
}

struct QuerySourceEntry<T, R, F>
where
    T: DeserializeOwned + Clone + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn() -> (MessageSchema, MessageSchema),
{
    inner: QuerySource<T, R>,
    schema_gen: F,
}

impl<T, R, F> QuerySourceEntryAny for QuerySourceEntry<T, R, F>
where
    T: DeserializeOwned + Clone + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn() -> (MessageSchema, MessageSchema) + Send + Sync + 'static,
{
    #[cfg(feature = "server")]
    fn query(
        &self,
        arg: &[u8],
    ) -> Result<(Action, Box<dyn ReplyReceiverAny>), DeserializationError> {
        ciborium::from_reader(arg).map(|arg| {
            let (action, receiver) = self.inner.query(arg);
            (action, Box::new(receiver) as Box<dyn ReplyReceiverAny>)
        })
    }
    #[cfg(feature = "server")]
    fn request_type_name(&self) -> &'static str {
        any::type_name::<T>()
    }
    #[cfg(feature = "server")]
    fn reply_type_name(&self) -> &'static str {
        any::type_name::<R>()
    }
    fn query_schema(&self) -> (MessageSchema, MessageSchema) {
        (self.schema_gen)()
    }
    fn get_query_source(&self) -> &dyn Any {
        &self.inner as &dyn Any
    }
    fn into_query_source(self: Box<Self>) -> Box<dyn Any> {
        Box::new(self.inner)
    }
}

#[cfg(feature = "server")]
/// A type-erased `ReplyReceiver` that returns CBOR-encoded replies.
pub(crate) trait ReplyReceiverAny {
    /// Take the replies, if any, encode them and collect them in a vector.
    fn take_collect(&mut self) -> Option<Result<Vec<Vec<u8>>, SerializationError>>;
}

#[cfg(feature = "server")]
impl<R: Serialize + 'static> ReplyReceiverAny for ReplyReceiver<R> {
    fn take_collect(&mut self) -> Option<Result<Vec<Vec<u8>>, SerializationError>> {
        let replies = self.take()?;

        let encoded_replies = (move || {
            let mut encoded_replies = Vec::new();
            for reply in replies {
                let mut encoded_reply = Vec::new();
                ciborium::into_writer(&reply, &mut encoded_reply)?;
                encoded_replies.push(encoded_reply);
            }

            Ok(encoded_replies)
        })();

        Some(encoded_replies)
    }
}
