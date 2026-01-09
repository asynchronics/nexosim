mod broadcaster;
mod sender;

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use futures_channel::oneshot;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::model::{Message, Model};
use crate::ports::InputFn;
use crate::simulation::{
    Address, DuplicateEventSourceError, DuplicateQuerySourceError, EventId, QueryId, SimInit,
};
use crate::util::unwrap_or_throw::UnwrapOrThrow;

pub(crate) use broadcaster::ReplyIterator;
use broadcaster::{EventBroadcaster, QueryBroadcaster};
use sender::{
    FilterMapInputSender, FilterMapReplierSender, InputSender, MapInputSender, MapReplierSender,
    ReplierSender,
};

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

    /// Converts an event source to an [`EventId`] that can later be used to
    /// schedule and process events within the simulation instance being built.
    ///
    /// This is typically only of interest when controlling the simulation from
    /// Rust. For simulations controlled by a remote client, use
    /// [`EventSource::bind_endpoint`] or [`EventSource::bind_raw_endpoint`].
    pub fn register(self, sim_init: &mut SimInit) -> EventId<T> {
        sim_init.link_event_source(self)
    }

    /// Adds an event source to the endpoint registry without requiring a
    /// [`Message`] implementation for its item type.
    ///
    /// If the specified name is already used by another input or another event
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`](crate::simulation::InitError).
    ///
    /// This is typically only of interest when controlling the simulation from
    /// a remote client or via the [Endpoints](crate::endpoints::Endpoints) API.
    /// In other cases, use [`EventSource::register`].
    pub fn bind_raw_endpoint(
        self,
        sim_init: &mut SimInit,
        name: impl Into<String>,
    ) -> Result<(), DuplicateEventSourceError<T>> {
        sim_init.add_event_source_raw(self, name)
    }

    /// Returns a ready to execute event future for the argument provided.
    /// This method can be e.g. used to spawn scheduled events from the queue.
    ///
    /// When processed, it broadcasts the event to all connected input
    /// ports.
    pub(crate) fn event_future(&self, arg: T) -> impl Future<Output = ()> + use<T> {
        let fut = self.broadcaster.broadcast(arg);

        async {
            fut.await.unwrap_or_throw();
        }
    }
}

impl<T: Message + Serialize + DeserializeOwned + Clone + Send + 'static> EventSource<T> {
    /// Adds this event source to the endpoint registry.
    ///
    /// If the specified name is already used by another input or another event
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`](crate::simulation::InitError).
    ///
    /// This is typically only of interest when controlling the simulation from
    /// a remote client or via the [Endpoints](crate::endpoints::Endpoints) API.
    /// In other cases, use [`EventSource::register`].
    pub fn bind_endpoint(
        self,
        sim_init: &mut SimInit,
        name: impl Into<String>,
    ) -> Result<(), DuplicateEventSourceError<T>> {
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

    /// Converts a query source to a [`QueryId`] that can later be used to
    /// schedule and process events within the simulation instance being built.
    ///
    /// This is typically only of interest when controlling the simulation from
    /// Rust. For simulations controlled by a remote client, use
    /// [`QuerySource::add_endpoint`] or [`QuerySource::add_endpoint_raw`].
    pub fn register(self, sim_init: &mut SimInit) -> QueryId<T, R> {
        sim_init.link_query_source(self)
    }

    pub(crate) fn query_future(
        &self,
        arg: T,
        replier: Option<ReplyWriter<R>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let fut = self.broadcaster.broadcast(arg);

        let fut = async move {
            let replies = fut.await.unwrap_or_throw();
            if let Some(replier) = replier {
                replier.send(replies);
            }
        };
        Box::pin(fut)
    }
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static, R: Serialize + Send + 'static>
    QuerySource<T, R>
{
    /// Adds a query source to the endpoint registry without requiring a
    /// [`Message`] implementation for its item type.
    ///
    /// If the specified name is already used by another query
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`](crate::simulation::InitError).
    ///
    /// This is typically only of interest when controlling the simulation from
    /// a remote client. For simulations controlled from Rust, use
    /// [`QuerySource::register`].
    pub fn add_endpoint_raw(
        self,
        name: impl Into<String>,
        sim_init: &mut SimInit,
    ) -> Result<(), DuplicateQuerySourceError<Self>> {
        sim_init.add_query_source_raw(self, name)
    }
}

impl<
    T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Message + Serialize + Send + 'static,
> QuerySource<T, R>
{
    /// Adds a query source to the endpoint registry.
    ///
    /// If the specified name is already used by another query
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`](crate::simulation::InitError).
    ///
    /// This is typically only of interest when controlling the simulation from
    /// a remote client. For simulations controlled from Rust, use
    /// [`QuerySource::register`].
    pub fn add_endpoint(
        self,
        sim_init: &mut SimInit,
        name: impl Into<String>,
    ) -> Result<(), DuplicateQuerySourceError<Self>> {
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

/// A typed consumer handle to a query reply channel.
#[derive(Debug)]
pub struct ReplyReader<R>(oneshot::Receiver<ReplyIterator<R>>);
impl<R: Send + 'static> ReplyReader<R> {
    /// A non blocking read attempt. If successful, returns an iterator over
    /// query replies.
    pub fn try_read(&mut self) -> Option<impl Iterator<Item = R>> {
        self.0.try_recv().ok()?
    }

    /// A blocking read. If successful, returns an iterator over query replies.
    /// Will return immediately with a `None` value if the channel has already
    /// been read.
    pub fn read(self) -> Option<impl Iterator<Item = R>> {
        pollster::block_on(self)
    }
}

impl<R: Send + 'static> Future for ReplyReader<R> {
    type Output = Option<ReplyIterator<R>>;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        Pin::new(&mut self.get_mut().0).poll(cx).map(Result::ok)
    }
}

pub(crate) struct ReplyWriter<R>(oneshot::Sender<ReplyIterator<R>>);
impl<R: Send + 'static> ReplyWriter<R> {
    pub(crate) fn send(self, reply: ReplyIterator<R>) {
        // TODO handle error
        let _ = self.0.send(reply);
    }
}

pub(crate) fn query_replier<R: Send + 'static>() -> (ReplyWriter<R>, ReplyReader<R>) {
    let (tx, rx) = oneshot::channel();
    (ReplyWriter(tx), ReplyReader(rx))
}
