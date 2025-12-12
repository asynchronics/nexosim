mod broadcaster;
mod sender;

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use crate::model::{Message, Model};
use crate::ports::InputFn;
use crate::simulation::{Action, Address, EventId, QueryId, ReplyWriter, SimInit};
use crate::util::slot;
use crate::util::unwrap_or_throw::UnwrapOrThrow;

pub(crate) use broadcaster::ReplyIterator;
use broadcaster::{EventBroadcaster, QueryBroadcaster};
use sender::{
    FilterMapInputSender, FilterMapReplierSender, InputSender, MapInputSender, MapReplierSender,
    ReplierSender,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::ReplierFn;

/// An event source port.
///
/// The `EventSource` port is similar to an [`Output`](crate::ports::Output)
/// port in that it can send events to connected input ports. It is not meant,
/// however, to be instantiated as a member of a model, but rather as a
/// simulation control endpoint instantiated during bench assembly.
pub struct EventSource<T: Serialize + DeserializeOwned + Clone + Send + 'static> {
    broadcaster: EventBroadcaster<T>,
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static> EventSource<T> {
    /// Creates a disconnected `EventSource` port.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a connection to an input port of the model specified by the
    /// address.
    ///
    /// The input port must be an asynchronous method of a model of type `M`
    /// taking as argument a value of type `T` plus, optionally, a scheduler
    /// reference.
    pub fn connect<M, F, S>(mut self, input: F, address: impl Into<Address<M>>) -> Self
    where
        M: Model,
        F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(InputSender::new(input, address.into().0));
        self.broadcaster.add(sender);
        self
    }

    /// Adds an auto-converting connection to an input port of the model
    /// specified by the address.
    ///
    /// Events are mapped to another type using the closure provided in
    /// argument.
    ///
    /// The input port must be an asynchronous method of a model of type `M`
    /// taking as argument a value of the type returned by the mapping closure
    /// plus, optionally, a context reference.
    pub fn map_connect<M, C, F, U, S>(
        mut self,
        map: C,
        input: F,
        address: impl Into<Address<M>>,
    ) -> Self
    where
        M: Model,
        C: for<'a> Fn(&'a T) -> U + Send + Sync + 'static,
        F: for<'a> InputFn<'a, M, U, S> + Sync + Clone,
        U: Send + 'static,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(MapInputSender::new(map, input, address.into().0));
        self.broadcaster.add(sender);
        self
    }

    /// Adds an auto-converting, filtered connection to an input port of the
    /// model specified by the address.
    ///
    /// Events are mapped to another type using the closure provided in
    /// argument, or ignored if the closure returns `None`.
    ///
    /// The input port must be an asynchronous method of a model of type `M`
    /// taking as argument a value of the type returned by the mapping closure
    /// plus, optionally, a context reference.
    pub fn filter_map_connect<M, C, F, U, S>(
        mut self,
        map: C,
        input: F,
        address: impl Into<Address<M>>,
    ) -> Self
    where
        M: Model,
        C: for<'a> Fn(&'a T) -> Option<U> + Send + Sync + 'static,
        F: for<'a> InputFn<'a, M, U, S> + Clone + Sync,
        U: Send + 'static,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(FilterMapInputSender::new(map, input, address.into().0));
        self.broadcaster.add(sender);
        self
    }

    pub fn register(self, sim_init: &mut SimInit) -> EventId<T> {
        sim_init.link_event_source(self)
    }

    pub fn add_raw(self, name: impl Into<String>, sim_init: &mut SimInit) -> Result<(), ()> {
        sim_init.add_event_source_raw(self, name)
    }

    /// Returns a ready to execute event future for the argument provided.
    /// This method can be e.g. used to spawn scheduled events from the queue.
    ///
    /// When processed, it broadcasts the event to all connected input
    /// ports.
    pub(crate) fn event_future(&self, arg: T) -> impl Future<Output = ()> {
        let fut = self.broadcaster.broadcast(arg);

        async {
            fut.await.unwrap_or_throw();
        }
    }

    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    pub fn action(&self, arg: T) -> Action {
        Action::new(Box::pin(self.event_future(arg)))
    }
}

impl<T: Message + Serialize + DeserializeOwned + Clone + Send + 'static> EventSource<T> {
    pub fn add(self, name: impl Into<String>, sim_init: &mut SimInit) -> Result<(), ()> {
        sim_init.add_event_source(self, name)
    }
}
impl<T: Serialize + DeserializeOwned + Clone + Send + 'static> Default for EventSource<T> {
    fn default() -> Self {
        Self {
            broadcaster: EventBroadcaster::default(),
        }
    }
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static> fmt::Debug for EventSource<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Event source ({} connected ports)",
            self.broadcaster.len()
        )
    }
}

pub(crate) struct RegisteredEventSource<T: Serialize + DeserializeOwned + Clone + Send + 'static> {
    inner: Arc<EventSource<T>>,
    pub(crate) event_id: EventId<T>,
}
impl<T: Serialize + DeserializeOwned + Clone + Send + 'static> RegisteredEventSource<T> {
    pub(crate) fn from_event_source(
        event_source: Arc<EventSource<T>>,
        event_id: EventId<T>,
    ) -> Self {
        Self {
            inner: event_source,
            event_id,
        }
    }
    pub(crate) fn action(&self, arg: T) -> Action {
        self.inner.action(arg)
    }
}

/// A query source port.
///
/// The `QuerySource` port is similar to an
/// [`Requestor`](crate::ports::Requestor) port in that it can send requests to
/// connected replier ports and receive replies. It is not meant, however, to be
/// instantiated as a member of a model, but rather as a simulation monitoring
/// endpoint instantiated during bench assembly.
pub struct QuerySource<T: Serialize + DeserializeOwned + Clone + Send + 'static, R: Send + 'static>
{
    broadcaster: QueryBroadcaster<T, R>,
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static, R: Send + 'static>
    QuerySource<T, R>
{
    /// Creates a disconnected `EventSource` port.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a connection to a replier port of the model specified by the
    /// address.
    ///
    /// The replier port must be an asynchronous method of a model of type `M`
    /// returning a value of type `R` and taking as argument a value of type `T`
    /// plus, optionally, a context reference.
    pub fn connect<M, F, S>(mut self, replier: F, address: impl Into<Address<M>>) -> Self
    where
        M: Model,
        F: for<'a> ReplierFn<'a, M, T, R, S> + Clone + Sync,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(ReplierSender::new(replier, address.into().0));
        self.broadcaster.add(sender);
        self
    }

    /// Adds an auto-converting connection to a replier port of the model
    /// specified by the address.
    ///
    /// Queries and replies are mapped to other types using the closures
    /// provided in argument.
    ///
    /// The replier port must be an asynchronous method of a model of type `M`
    /// returning a value of the type returned by the reply mapping closure and
    /// taking as argument a value of the type returned by the query mapping
    /// closure plus, optionally, a context reference.
    pub fn map_connect<M, C, D, F, U, Q, S>(
        mut self,
        query_map: C,
        reply_map: D,
        replier: F,
        address: impl Into<Address<M>>,
    ) -> Self
    where
        M: Model,
        C: for<'a> Fn(&'a T) -> U + Send + Sync + 'static,
        D: Fn(Q) -> R + Send + Sync + 'static,
        F: for<'a> ReplierFn<'a, M, U, Q, S> + Clone + Sync,
        U: Send + 'static,
        Q: Send + 'static,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(MapReplierSender::new(
            query_map,
            reply_map,
            replier,
            address.into().0,
        ));
        self.broadcaster.add(sender);
        self
    }

    /// Adds an auto-converting, filtered connection to a replier port of the
    /// model specified by the address.
    ///
    /// Queries and replies are mapped to other types using the closures
    /// provided in argument, or ignored if the query closure returns `None`.
    ///
    /// The replier port must be an asynchronous method of a model of type `M`
    /// returning a value of the type returned by the reply mapping closure and
    /// taking as argument a value of the type returned by the query mapping
    /// closure plus, optionally, a context reference.
    pub fn filter_map_connect<M, C, D, F, U, Q, S>(
        mut self,
        query_filter_map: C,
        reply_map: D,
        replier: F,
        address: impl Into<Address<M>>,
    ) -> Self
    where
        M: Model,
        C: for<'a> Fn(&'a T) -> Option<U> + Send + Sync + 'static,
        D: Fn(Q) -> R + Send + Sync + 'static,
        F: for<'a> ReplierFn<'a, M, U, Q, S> + Clone + Sync,
        U: Send + 'static,
        Q: Send + 'static,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(FilterMapReplierSender::new(
            query_filter_map,
            reply_map,
            replier,
            address.into().0,
        ));
        self.broadcaster.add(sender);
        self
    }

    pub fn register(self, sim_init: &mut SimInit) -> QueryId<T, R> {
        sim_init.link_query_source(self)
    }

    /// Returns an action which, when processed, broadcasts a query to all
    /// connected replier ports.
    pub fn query(&self, arg: T) -> (Action, ReplyReceiver<R>) {
        let (writer, reader) = slot::slot();
        let fut = self.broadcaster.broadcast(arg);
        let fut = async move {
            let replies = fut.await.unwrap_or_throw();
            let _ = writer.write(replies);
        };

        (Action::new(Box::pin(fut)), ReplyReceiver::<R>(reader))
    }

    pub(crate) fn query_future(
        &self,
        arg: T,
        replier: ReplyWriter<R>,
    ) -> impl Future<Output = ()> + Send {
        let fut = self.broadcaster.broadcast(arg);

        async move {
            let replies = fut.await.unwrap_or_throw();
            replier.send(replies);
        }
    }
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static, R: Serialize + Send + 'static>
    QuerySource<T, R>
{
    pub fn add_raw(self, name: impl Into<String>, sim_init: &mut SimInit) -> Result<(), ()> {
        sim_init.add_query_source_raw(self, name)
    }
}

impl<
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    > QuerySource<T, R>
{
    pub fn add(self, name: impl Into<String>, sim_init: &mut SimInit) -> Result<(), ()> {
        sim_init.add_query_source(self, name)
    }
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static, R: Send + 'static> Default
    for QuerySource<T, R>
{
    fn default() -> Self {
        Self {
            broadcaster: QueryBroadcaster::default(),
        }
    }
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static, R: Send + 'static> fmt::Debug
    for QuerySource<T, R>
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Query source ({} connected ports)",
            self.broadcaster.len()
        )
    }
}

pub struct RegisteredQuerySource<
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Send + 'static,
> {
    inner: Arc<QuerySource<T, R>>,
    pub(crate) query_id: QueryId<T, R>,
}
impl<T, R> RegisteredQuerySource<T, R>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Send + 'static,
{
    pub(crate) fn from_event_source(
        query_source: Arc<QuerySource<T, R>>,
        query_id: QueryId<T, R>,
    ) -> Self {
        Self {
            inner: query_source,
            query_id,
        }
    }
    pub(crate) fn query(&self, arg: T) -> (Action, ReplyReceiver<R>) {
        self.inner.query(arg)
    }
}

/// A receiver for all replies collected from a single query broadcast.
pub struct ReplyReceiver<R>(slot::SlotReader<ReplyIterator<R>>);

impl<R> ReplyReceiver<R> {
    /// Returns all replies to a query.
    ///
    /// Returns `None` if the replies are not yet available or if they were
    /// already taken in a previous call to `take`.
    pub fn take(&mut self) -> Option<impl Iterator<Item = R>> {
        self.0.try_read().ok()
    }
}

impl<R> fmt::Debug for ReplyReceiver<R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Replies")
    }
}
