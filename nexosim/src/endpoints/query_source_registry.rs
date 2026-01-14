use std::any;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::marker::PhantomData;

#[cfg(feature = "server")]
use ciborium;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::path::Path;
use crate::ports::QuerySource;
use crate::simulation::{QueryId, QueryIdErased, SchedulerRegistry};

#[cfg(feature = "server")]
use crate::ports::{ReplyReader, ReplyWriter};

use super::{EndpointError, Message, MessageSchema};

#[cfg(feature = "server")]
type SerializationError = ciborium::ser::Error<std::io::Error>;
#[cfg(feature = "server")]
type DeserializationError = ciborium::de::Error<std::io::Error>;

/// A registry that holds all query sources meant to be accessed through remote
/// procedure calls.
#[derive(Default)]
pub(crate) struct QuerySourceRegistry(HashMap<Path, Box<dyn QuerySourceEntryAny>>);

impl QuerySourceRegistry {
    /// Adds a query source to the registry.
    ///
    /// If the specified path is already used by another query source, the path
    /// to the query and the query source itself are returned in the error.
    pub(crate) fn add<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        path: Path,
        registry: &mut SchedulerRegistry,
    ) -> Result<(), (Path, QuerySource<T, R>)>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.add_any(source, path, || (T::schema(), R::schema()), registry)
    }

    /// Adds a query source to the registry without a schema definition.
    ///
    /// If the specified path is already used by another query source, the path
    /// to the query and the query source itself are returned in the error.
    pub(crate) fn add_raw<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        path: Path,
        registry: &mut SchedulerRegistry,
    ) -> Result<(), (Path, QuerySource<T, R>)>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.add_any(source, path, || (String::new(), String::new()), registry)
    }

    /// Adds a query source to the registry, possibly with an empty schema
    /// definition.
    fn add_any<T, R, F>(
        &mut self,
        source: QuerySource<T, R>,
        path: Path,
        schema_gen: F,
        registry: &mut SchedulerRegistry,
    ) -> Result<(), (Path, QuerySource<T, R>)>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
        F: Fn() -> (MessageSchema, MessageSchema) + Send + Sync + 'static,
    {
        match self.0.entry(path) {
            Entry::Vacant(s) => {
                let query_id = registry.add_query_source(source);
                let entry = QuerySourceEntry {
                    inner: query_id,
                    schema_gen,
                };
                s.insert(Box::new(entry));

                Ok(())
            }
            Entry::Occupied(e) => Err((e.key().clone(), source)),
        }
    }

    /// Returns a reference to the specified query source if it is in the
    /// registry.
    pub(crate) fn get(&self, path: &Path) -> Result<&dyn QuerySourceEntryAny, EndpointError> {
        self.0
            .get(path)
            .map(|s| s.as_ref())
            .ok_or_else(|| EndpointError::QuerySourceNotFound { path: path.clone() })
    }

    /// Returns the query identifier of the requested query source if it is in
    /// the registry and is of the proper type.
    pub(crate) fn get_source_id<T, R>(&self, path: &Path) -> Result<QueryId<T, R>, EndpointError>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        let query_id = self.get(path).and_then(|entry| {
            if entry.request_type_id() == TypeId::of::<T>()
                && entry.reply_type_id() == TypeId::of::<R>()
            {
                Ok(entry.get_query_id())
            } else {
                Err(EndpointError::InvalidQuerySourceType {
                    path: path.clone(),
                    found_request_type: any::type_name::<T>(),
                    found_reply_type: any::type_name::<R>(),
                    expected_request_type: entry.request_type_name(),
                    expected_reply_type: entry.reply_type_name(),
                })
            }
        })?;

        Ok(QueryId(query_id.0, PhantomData))
    }

    /// Returns an iterator over the paths of all registered query sources.
    pub(crate) fn list_sources(&self) -> impl Iterator<Item = &Path> {
        self.0.keys()
    }

    /// Returns the request and reply schemas of the specified query source, in
    /// this order, if they are in the registry.
    pub(crate) fn get_source_schema(
        &self,
        path: &Path,
    ) -> Result<(MessageSchema, MessageSchema), EndpointError> {
        Ok(self.get(path)?.query_schema())
    }

    /// Returns an iterator over the paths and schemas of all registered query
    /// sources.
    #[cfg(feature = "server")]
    pub(crate) fn list_schemas(
        &self,
    ) -> impl Iterator<Item = (&Path, MessageSchema, MessageSchema)> {
        self.0.iter().map(|(path, src)| {
            let schemas = src.query_schema();
            (path, schemas.0, schemas.1)
        })
    }
}

impl fmt::Debug for QuerySourceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "QuerySourceRegistry ({} query sources)", self.0.len(),)
    }
}

/// A type-erased query source that operates on CBOR-encoded serialized queries
/// and returns CBOR-encoded replies.
pub(crate) trait QuerySourceEntryAny: Any + Send + Sync + 'static {
    /// Returns a type erased deserialized query argument.
    ///
    /// The argument is expected to conform to the serde CBOR encoding.
    #[cfg(feature = "server")]
    fn deserialize_arg(&self, serialized_arg: &[u8]) -> Result<Box<dyn Any>, DeserializationError>;

    /// The `TypeId` of the request event.
    fn request_type_id(&self) -> TypeId;

    /// Human-readable name of the request type, as returned by
    /// `any::type_name`.
    fn request_type_name(&self) -> &'static str;

    /// The `TypeId` of the reply event.
    fn reply_type_id(&self) -> TypeId;

    /// Human-readable name of the reply type, as returned by `any::type_name`.
    fn reply_type_name(&self) -> &'static str;

    /// Returns the request and reply schemas of the query source, in this
    /// order.
    ///
    /// If the query was added via the `add_raw` method, it returns empty schema
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
    fn request_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    fn request_type_name(&self) -> &'static str {
        any::type_name::<T>()
    }
    fn reply_type_id(&self) -> TypeId {
        TypeId::of::<R>()
    }
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
/// A type-erased reply writer.
pub(crate) trait ReplyWriterAny: Any + Send {}

#[cfg(feature = "server")]
impl<R: Send + 'static> ReplyWriterAny for ReplyWriter<R> {}

#[cfg(feature = "server")]
/// A type-erased reply reader that returns CBOR-encoded replies.
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
