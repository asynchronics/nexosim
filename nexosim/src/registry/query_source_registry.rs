use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;

use ciborium;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::ports::{QuerySource, ReplyReceiver};

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
        T: DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        match self.0.entry(name.into()) {
            Entry::Vacant(s) => {
                s.insert(Box::new(source));

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
    // fn query(
    //     &self,
    //     arg: &[u8],
    // ) -> Result<(Action, Box<dyn ReplyReceiverAny>), DeserializationError>;

    fn into_future(
        &self,
        arg: &[u8],
    ) -> Result<(Box<dyn Future<Output = ()>>, Box<dyn ReplyReceiverAny>), DeserializationError>;

    /// Human-readable name of the request type, as returned by
    /// `any::type_name`.
    fn request_type_name(&self) -> &'static str;

    /// Human-readable name of the reply type, as returned by
    /// `any::type_name`.
    fn reply_type_name(&self) -> &'static str;
}

impl<T, R> QuerySourceAny for QuerySource<T, R>
where
    T: DeserializeOwned + Clone + Send + 'static,
    R: Serialize + Send + 'static,
{
    // fn query(
    //     &self,
    //     arg: &[u8],
    // ) -> Result<(Action, Box<dyn ReplyReceiverAny>), DeserializationError> {
    //     ciborium::from_reader(arg).map(|arg| {
    //         let (action, reply_recv) = self.query(arg);
    //         let reply_recv: Box<dyn ReplyReceiverAny> = Box::new(reply_recv);

    //         (action, reply_recv)
    //     })
    // }

    fn into_future(
        &self,
        arg: &[u8],
    ) -> Result<(Box<dyn Future<Output = ()>>, Box<dyn ReplyReceiverAny>), DeserializationError>
    {
        ciborium::from_reader(arg).map(|arg| {
            let (fut, reply_recv) = self.into_future(arg);
            let reply_recv: Box<dyn ReplyReceiverAny + 'static> = Box::new(reply_recv);
            let fut: Box<dyn Future<Output = ()> + 'static> = Box::new(fut);

            (fut, reply_recv)
        })
    }

    fn request_type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn reply_type_name(&self) -> &'static str {
        std::any::type_name::<R>()
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
