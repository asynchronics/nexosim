use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;

use ciborium;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::ports::{QuerySource, ReplyReceiver};
use crate::simulation::Action;

use super::{Message, MessageSchema, RegistryError};

type DeserializationError = ciborium::de::Error<std::io::Error>;
type SerializationError = ciborium::ser::Error<std::io::Error>;

/// A registry that holds all sources and sinks meant to be accessed through
/// remote procedure calls.
#[derive(Default)]
pub(crate) struct QuerySourceRegistry(HashMap<String, Box<dyn QuerySourceAny>>);

impl QuerySourceRegistry {
    /// Adds a query source to the registry.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), QuerySource<T, R>>
    where
        T: Message + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.insert_entry(source, name, || (T::schema(), R::schema()))
    }

    /// Adds a query source to the registry without a schema definition.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_raw<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), QuerySource<T, R>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.insert_entry(source, name, || (String::new(), String::new()))
    }

    fn insert_entry<T, R, F>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
        schema_gen: F,
    ) -> Result<(), QuerySource<T, R>>
    where
        T: DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
        F: Fn() -> (MessageSchema, MessageSchema) + Send + Sync + 'static,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                let entry = QuerySourceEntry {
                    inner: source,
                    schema_gen,
                };
                s.insert(Box::new(entry));

                Ok(())
            }
            Entry::Occupied(_) => Err(source),
        }
    }

    /// Returns a mutable reference to the specified query source if it is in
    /// the registry.
    pub(crate) fn get(&self, name: &str) -> Option<&dyn QuerySourceAny> {
        self.0.get(name).map(|s| s.as_ref())
    }

    /// Returns an iterator over the names of the registered query sources.
    pub(crate) fn list_sources(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    /// Returns the input and output schemas of a specified query source if it
    /// is in the registry.
    pub(crate) fn get_source_schema(
        &self,
        name: &str,
    ) -> Result<(MessageSchema, MessageSchema), RegistryError> {
        Ok(self
            .get(name)
            .ok_or(RegistryError::SourceNotFound(name.to_string()))?
            .get_schema())
    }
}

impl fmt::Debug for QuerySourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "QuerySourceRegistry ({} query sources)", self.0.len(),)
    }
}

/// A type-erased `QuerySource` that operates on CBOR-encoded serialized queries
/// and returns CBOR-encoded replies.
pub(crate) trait QuerySourceAny: Send + Sync + 'static {
    /// Returns an action which, when processed, broadcasts a query to all
    /// connected replier ports.
    ///
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    fn query(
        &self,
        arg: &[u8],
    ) -> Result<(Action, Box<dyn ReplyReceiverAny>), DeserializationError>;

    /// Human-readable name of the request type, as returned by
    /// `any::type_name`.
    fn request_type_name(&self) -> &'static str;

    /// Human-readable name of the reply type, as returned by
    /// `any::type_name`.
    fn reply_type_name(&self) -> &'static str;

    /// Returns the input and output schemas of the query source.
    ///
    /// The first element of the tuple being the input schema, the second one
    /// the output schema.
    /// If the query was added via `add_raw` method, it returns an empty schema
    /// strings.
    fn get_schema(&self) -> (MessageSchema, MessageSchema);
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

impl<T, R, F> QuerySourceAny for QuerySourceEntry<T, R, F>
where
    T: DeserializeOwned + Clone + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn() -> (MessageSchema, MessageSchema) + Send + Sync + 'static,
{
    fn query(
        &self,
        arg: &[u8],
    ) -> Result<(Action, Box<dyn ReplyReceiverAny>), DeserializationError> {
        ciborium::from_reader(arg).map(|arg| {
            let (action, reply_recv) = self.inner.query(arg);
            let reply_recv: Box<dyn ReplyReceiverAny> = Box::new(reply_recv);

            (action, reply_recv)
        })
    }

    fn request_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn reply_type_name(&self) -> &'static str {
        std::any::type_name::<R>()
    }

    fn get_schema(&self) -> (MessageSchema, MessageSchema) {
        (self.schema_gen)()
    }
}

/// A type-erased `ReplyReceiver` that returns CBOR-encoded replies.
pub(crate) trait ReplyReceiverAny {
    /// Take the replies, if any, encode them and collect them in a vector.
    fn take_collect(&mut self) -> Option<Result<Vec<Vec<u8>>, SerializationError>>;
}

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
