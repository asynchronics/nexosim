use std::any::{self, Any};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

#[cfg(feature = "server")]
use ciborium;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::simulation::QueryId;
use crate::simulation::QueryIdErased;

#[cfg(feature = "server")]
use crate::ports::{ReplyReader, ReplyWriter};

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
    pub(crate) fn add<T, R>(&mut self, query_id: QueryId<T, R>, name: String) -> Result<(), ()>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.add_any(query_id, name, || (T::schema(), R::schema()))
    }

    /// Adds a query source to the registry without a schema definition.
    ///
    /// If the specified name is already used by another query source, the name
    /// of the query and the query source itself are returned in the error.
    pub(crate) fn add_raw<T, R>(&mut self, query_id: QueryId<T, R>, name: String) -> Result<(), ()>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.add_any(query_id, name, || (String::new(), String::new()))
    }

    // FIXME error type
    /// Adds a query source to the registry, possibly with an empty schema
    /// definition.
    fn add_any<T, R, F>(
        &mut self,
        query_id: QueryId<T, R>,
        name: String,
        schema_gen: F,
    ) -> Result<(), ()>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
        F: Fn() -> (MessageSchema, MessageSchema) + Send + Sync + 'static,
    {
        match self.0.entry(name) {
            Entry::Vacant(s) => {
                let entry = QuerySourceEntry {
                    inner: query_id,
                    schema_gen,
                };
                s.insert(Box::new(entry));

                Ok(())
            }
            Entry::Occupied(_) => Err(()),
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

    /// Returns a typed SourceId of the requested EventSource.
    pub(crate) fn get_source_id<T, R>(&self, name: &str) -> Result<QueryId<T, R>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        let query_id = self.get(name)?.get_query_id();
        Ok(QueryId(query_id.0, std::marker::PhantomData))
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
    /// Returns a type erased deserialized query argument.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn deserialize_arg(&self, serialized_arg: &[u8]) -> Result<Box<dyn Any>, DeserializationError>;

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

    /// Returns type erased QueryId.
    fn get_query_id(&self) -> QueryIdErased;

    #[cfg(feature = "server")]
    fn replier(&self) -> (Box<dyn ReplyWriterAny>, Box<dyn ReplyReaderAny>);
}

struct QuerySourceEntry<T, R, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn() -> (MessageSchema, MessageSchema),
{
    inner: QueryId<T, R>,
    schema_gen: F,
}

impl<T, R, F> QuerySourceEntryAny for QuerySourceEntry<T, R, F>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Serialize + Send + 'static,
    F: Fn() -> (MessageSchema, MessageSchema) + Send + Sync + 'static,
{
    #[cfg(feature = "server")]
    fn deserialize_arg(&self, serialized_arg: &[u8]) -> Result<Box<dyn Any>, DeserializationError> {
        ciborium::from_reader(serialized_arg).map(|arg: T| Box::new(arg) as Box<dyn Any>)
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
    fn get_query_id(&self) -> QueryIdErased {
        self.inner.into()
    }
    #[cfg(feature = "server")]
    fn replier(&self) -> (Box<dyn ReplyWriterAny>, Box<dyn ReplyReaderAny>) {
        use crate::ports::query_replier;

        let (tx, rx) = query_replier::<R>();
        (Box::new(tx), Box::new(rx))
    }
}

#[cfg(feature = "server")]
/// A type-erased `ReplySender`
pub(crate) trait ReplyWriterAny: Any + Send {}

#[cfg(feature = "server")]
impl<R: Send + 'static> ReplyWriterAny for ReplyWriter<R> {}

#[cfg(feature = "server")]
/// A type-erased `ReplyReceiver` that returns CBOR-encoded replies.
pub(crate) trait ReplyReaderAny {
    /// Take the replies, if any, encode them and collect them in a vector.
    fn take_collect(&mut self) -> Option<Result<Vec<Vec<u8>>, SerializationError>>;
}

#[cfg(feature = "server")]
impl<R: Serialize + Send + 'static> ReplyReaderAny for ReplyReader<R> {
    fn take_collect(&mut self) -> Option<Result<Vec<Vec<u8>>, SerializationError>> {
        let replies = self.try_read()?;

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
