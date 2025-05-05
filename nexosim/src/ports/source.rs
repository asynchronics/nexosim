mod broadcaster;
mod sender;

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::model::Model;
use crate::ports::InputFn;
use crate::simulation::{
    process_event, send_keyed_event, Action, Address, EventKey, KeyedOnceAction,
    KeyedPeriodicAction, OnceAction, PeriodicAction,
};
use crate::util::slot;
use crate::util::unwrap_or_throw::UnwrapOrThrow;

use broadcaster::{EventBroadcaster, QueryBroadcaster, ReplyIterator};
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
pub struct EventSource<T: Clone + Send + 'static> {
    broadcaster: EventBroadcaster<T>,
}

impl<T: Clone + Send + 'static> EventSource<T> {
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
    pub fn connect<M, F, S>(&mut self, input: F, address: impl Into<Address<M>>)
    where
        M: Model,
        F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(InputSender::new(input, address.into().0));
        self.broadcaster.add(sender);
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
    pub fn map_connect<M, C, F, U, S>(&mut self, map: C, input: F, address: impl Into<Address<M>>)
    where
        M: Model,
        C: for<'a> Fn(&'a T) -> U + Send + Sync + 'static,
        F: for<'a> InputFn<'a, M, U, S> + Sync + Clone,
        U: Send + 'static,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(MapInputSender::new(map, input, address.into().0));
        self.broadcaster.add(sender);
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
        &mut self,
        map: C,
        input: F,
        address: impl Into<Address<M>>,
    ) where
        M: Model,
        C: for<'a> Fn(&'a T) -> Option<U> + Send + Sync + 'static,
        F: for<'a> InputFn<'a, M, U, S> + Clone + Sync,
        U: Send + 'static,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(FilterMapInputSender::new(map, input, address.into().0));
        self.broadcaster.add(sender);
    }

    /// Returns an action which, when processed, broadcasts an event to all
    /// connected input ports.
    pub fn event(&self, arg: T) -> Action {
        let fut = self.broadcaster.broadcast(arg);
        let fut = async {
            fut.await.unwrap_or_throw();
        };

        Action::new(OnceAction::new(fut))
    }

    /// Returns a cancellable action and a cancellation key; when processed, the
    /// action broadcasts an event to all connected input ports.
    pub fn keyed_event(&self, arg: T) -> (Action, EventKey) {
        let action_key = EventKey::new();
        let fut = self.broadcaster.broadcast(arg);

        let action = Action::new(KeyedOnceAction::new(
            // Cancellation is ignored once the action is already spawned on the
            // executor. This means the action cannot be cancelled once the
            // simulation step targeted by the action is running, but since an
            // event source is meant to be used outside the simulator, this
            // shouldn't be an issue in practice.
            |_| async {
                fut.await.unwrap_or_throw();
            },
            action_key.clone(),
        ));

        (action, action_key)
    }

    /// Returns a periodically recurring action which, when processed,
    /// broadcasts an event to all connected input ports.
    pub fn periodic_event(self: &Arc<Self>, period: Duration, arg: T) -> Action {
        let source = self.clone();

        Action::new(PeriodicAction::new(
            || async move {
                let fut = source.broadcaster.broadcast(arg);
                fut.await.unwrap_or_throw();
            },
            period,
        ))
    }

    /// Returns a cancellable, periodically recurring action and a cancellation
    /// key; when processed, the action broadcasts an event to all connected
    /// input ports.
    pub fn keyed_periodic_event(self: &Arc<Self>, period: Duration, arg: T) -> (Action, EventKey) {
        let action_key = EventKey::new();
        let source = self.clone();

        let action = Action::new(KeyedPeriodicAction::new(
            // Cancellation is ignored once the action is already spawned on the
            // executor. This means the action cannot be cancelled while the
            // simulation is running, but since an event source is meant to be
            // used outside the simulator, this shouldn't be an issue in
            // practice.
            |_| async move {
                let fut = source.broadcaster.broadcast(arg);
                fut.await.unwrap_or_throw();
            },
            period,
            action_key.clone(),
        ));

        (action, action_key)
    }

    pub(crate) fn into_future(
        &self,
        arg: T,
        event_key: Option<EventKey>,
    ) -> impl Future<Output = ()> {
        let fut = self.broadcaster.broadcast(arg);

        async {
            match event_key {
                Some(key) if key.is_cancelled() => (),
                _ => fut.await.unwrap_or_throw(),
            };
        }
    }
}

impl<T: Clone + Send + 'static> Default for EventSource<T> {
    fn default() -> Self {
        Self {
            broadcaster: EventBroadcaster::default(),
        }
    }
}

impl<T: Clone + Send + 'static> fmt::Debug for EventSource<T> {
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
pub struct QuerySource<T: Clone + Send + 'static, R: Send + 'static> {
    broadcaster: QueryBroadcaster<T, R>,
}

impl<T: Clone + Send + 'static, R: Send + 'static> QuerySource<T, R> {
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
    pub fn connect<M, F, S>(&mut self, replier: F, address: impl Into<Address<M>>)
    where
        M: Model,
        F: for<'a> ReplierFn<'a, M, T, R, S> + Clone + Sync,
        S: Send + Sync + 'static,
    {
        let sender = Box::new(ReplierSender::new(replier, address.into().0));
        self.broadcaster.add(sender);
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
        &mut self,
        query_map: C,
        reply_map: D,
        replier: F,
        address: impl Into<Address<M>>,
    ) where
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
        &mut self,
        query_filter_map: C,
        reply_map: D,
        replier: F,
        address: impl Into<Address<M>>,
    ) where
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

        let action = Action::new(OnceAction::new(fut));

        (action, ReplyReceiver::<R>(reader))
    }
}

impl<T: Clone + Send + 'static, R: Send + 'static> Default for QuerySource<T, R> {
    fn default() -> Self {
        Self {
            broadcaster: QueryBroadcaster::default(),
        }
    }
}

impl<T: Clone + Send + 'static, R: Send + 'static> fmt::Debug for QuerySource<T, R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Query source ({} connected ports)",
            self.broadcaster.len()
        )
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
