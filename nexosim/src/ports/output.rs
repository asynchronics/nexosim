mod broadcaster;
mod sender;

use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;

use schemars::schema_for;
use serde::{Deserialize, Serialize};

use crate::macros::scoped_thread_local::scoped_thread_local;
use crate::model::Model;
use crate::ports::EventSink;
use crate::ports::{InputFn, ReplierFn};
use crate::simulation::Address;
use crate::util::cached_rw_lock::CachedRwLock;
use crate::util::unwrap_or_throw::UnwrapOrThrow;

use broadcaster::{EventBroadcaster, QueryBroadcaster};
use sender::{FilterMapReplierSender, Sender};

use self::sender::{
    EventSinkSender, FilterMapEventSinkSender, FilterMapInputSender, InputSender,
    MapEventSinkSender, MapInputSender, MapReplierSender, ReplierSender,
};

scoped_thread_local!(pub(crate) static PORT_REG: PortsReg);
pub(crate) type PortsReg = Mutex<VecDeque<Box<dyn PortAny>>>;

pub(crate) trait PortAny: Any {
    fn tag(&self) -> Option<&String> {
        None
    }
    fn type_name(&self) -> &'static str {
        "todo"
    }
    fn schema(&self) -> Option<schemars::Schema> {
        None
    }
}

impl<T: schemars::JsonSchema + Clone + Send + 'static> PortAny for Output<T> {
    fn tag(&self) -> Option<&String> {
        self.tag.as_ref()
    }
    fn type_name(&self) -> &'static str {
        std::any::type_name_of_val(self)
    }
    fn schema(&self) -> Option<schemars::Schema> {
        Some(schemars::schema_for!(T))
    }
}
impl<T: Clone + Send + 'static, R: Send + 'static> PortAny for Requestor<T, R> {}
impl<T: Clone + Send + 'static, R: Send + 'static> PortAny for UniRequestor<T, R> {}

/// An output port.
///
/// `Output` ports can be connected to input ports, i.e. to asynchronous model
/// methods that return no value. They broadcast events to all connected input
/// ports.
///
/// When an `Output` is cloned, the information on connected ports remains
/// shared and therefore all clones use and modify the same list of connected
/// ports.
#[derive(Clone)]
pub struct Output<T: Clone + Send + 'static> {
    broadcaster: CachedRwLock<EventBroadcaster<T>>,
    tag: Option<String>,
}

impl<T: Clone + Send + 'static> Output<T> {
    /// Creates a disconnected `Output` port.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tagged(tag: String) -> Self {
        let mut output = Self::new();
        output.tag = Some(tag);
        output
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
        F: for<'a> InputFn<'a, M, T, S> + Clone,
        S: Send + 'static,
    {
        let sender = Box::new(InputSender::new(input, address.into().0));
        self.broadcaster.write().unwrap().add(sender);
    }

    /// Adds a connection to an event sink such as an
    /// [`EventSlot`](crate::ports::EventSlot) or
    /// [`EventQueue`](crate::ports::EventQueue).
    pub fn connect_sink<S: EventSink<T>>(&mut self, sink: &S) {
        let sender = Box::new(EventSinkSender::new(sink.writer()));
        self.broadcaster.write().unwrap().add(sender)
    }

    /// Adds an auto-converting connection to an input port of the model
    /// specified by the address.
    ///
    /// Events are mapped to another type using the closure provided in
    /// argument.
    ///
    /// The input port must be an asynchronous method of a model of type `M`
    /// taking as argument a value of the type returned by the mapping
    /// closure plus, optionally, a context reference.
    pub fn map_connect<M, C, F, U, S>(&mut self, map: C, input: F, address: impl Into<Address<M>>)
    where
        M: Model,
        C: Fn(&T) -> U + Send + Sync + 'static,
        F: for<'a> InputFn<'a, M, U, S> + Clone,
        U: Send + 'static,
        S: Send + 'static,
    {
        let sender = Box::new(MapInputSender::new(map, input, address.into().0));
        self.broadcaster.write().unwrap().add(sender);
    }

    /// Adds an auto-converting connection to an event sink such as an
    /// [`EventSlot`](crate::ports::EventSlot) or
    /// [`EventQueue`](crate::ports::EventQueue).
    ///
    /// Events are mapped to another type using the closure provided in
    /// argument.
    pub fn map_connect_sink<C, U, S>(&mut self, map: C, sink: &S)
    where
        C: Fn(&T) -> U + Send + Sync + 'static,
        U: Send + 'static,
        S: EventSink<U>,
    {
        let sender = Box::new(MapEventSinkSender::new(map, sink.writer()));
        self.broadcaster.write().unwrap().add(sender);
    }

    /// Adds an auto-converting, filtered connection to an input port of the
    /// model specified by the address.
    ///
    /// Events are mapped to another type using the closure provided in
    /// argument, or ignored if the closure returns `None`.
    ///
    /// The input port must be an asynchronous method of a model of type `M`
    /// taking as argument a value of the type returned by the mapping
    /// closure plus, optionally, a context reference.
    pub fn filter_map_connect<M, C, F, U, S>(
        &mut self,
        filter_map: C,
        input: F,
        address: impl Into<Address<M>>,
    ) where
        M: Model,
        C: Fn(&T) -> Option<U> + Send + Sync + 'static,
        F: for<'a> InputFn<'a, M, U, S> + Clone,
        U: Send + 'static,
        S: Send + 'static,
    {
        let sender = Box::new(FilterMapInputSender::new(
            filter_map,
            input,
            address.into().0,
        ));
        self.broadcaster.write().unwrap().add(sender);
    }

    /// Adds an auto-converting connection to an event sink such as an
    /// [`EventSlot`](crate::ports::EventSlot) or
    /// [`EventQueue`](crate::ports::EventQueue).
    ///
    /// Events are mapped to another type using the closure provided in
    /// argument.
    pub fn filter_map_connect_sink<C, U, S>(&mut self, filter_map: C, sink: &S)
    where
        C: Fn(&T) -> Option<U> + Send + Sync + 'static,
        U: Send + 'static,
        S: EventSink<U>,
    {
        let sender = Box::new(FilterMapEventSinkSender::new(filter_map, sink.writer()));
        self.broadcaster.write().unwrap().add(sender);
    }

    /// Broadcasts an event to all connected input ports.
    pub async fn send(&mut self, arg: T) {
        let broadcaster = self.broadcaster.write_scratchpad().unwrap();
        broadcaster.broadcast(arg).await.unwrap_or_throw();
    }
}

impl<T: Clone + Send + 'static> Default for Output<T> {
    fn default() -> Self {
        Self {
            broadcaster: CachedRwLock::new(EventBroadcaster::default()),
            tag: None,
        }
    }
}

impl<T: Clone + Send + 'static> fmt::Debug for Output<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Output ({} connected ports)",
            self.broadcaster.read_unsync().len()
        )
    }
}

impl<T: schemars::JsonSchema + Clone + Send> Serialize for Output<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Use serialization to store output copy in the global registry.
        PORT_REG.map(|reg| reg.lock().unwrap().push_back(Box::new(self.clone())));
        serializer.serialize_unit()
    }
}

impl<'de, T: Clone + Send> Deserialize<'de> for Output<T> {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Pop stored value from the registry.
        // If registry is not present, use a default
        Ok(*PORT_REG
            .map(|reg| {
                (reg.lock().unwrap().pop_front().unwrap() as Box<dyn Any>)
                    .downcast()
                    .unwrap()
            })
            .unwrap_or_default())
    }
}

/// A requestor port.
///
/// `Requestor` ports can be connected to replier ports, i.e. to asynchronous
/// model methods that return a value. They broadcast queries to all connected
/// replier ports.
///
/// When a `Requestor` is cloned, the information on connected ports remains
/// shared and therefore all clones use and modify the same list of connected
/// ports.
pub struct Requestor<T: Clone + Send + 'static, R: Send + 'static> {
    broadcaster: CachedRwLock<QueryBroadcaster<T, R>>,
}

impl<T: Clone + Send + 'static, R: Send + 'static> Requestor<T, R> {
    /// Creates a disconnected `Requestor` port.
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
        F: for<'a> ReplierFn<'a, M, T, R, S> + Clone,
        S: Send + 'static,
    {
        let sender = Box::new(ReplierSender::new(replier, address.into().0));
        self.broadcaster.write().unwrap().add(sender);
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
        C: Fn(&T) -> U + Send + Sync + 'static,
        D: Fn(Q) -> R + Send + Sync + 'static,
        F: for<'a> ReplierFn<'a, M, U, Q, S> + Clone,
        U: Send + 'static,
        Q: Send + 'static,
        S: Send + 'static,
    {
        let sender = Box::new(MapReplierSender::new(
            query_map,
            reply_map,
            replier,
            address.into().0,
        ));
        self.broadcaster.write().unwrap().add(sender);
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
        C: Fn(&T) -> Option<U> + Send + Sync + 'static,
        D: Fn(Q) -> R + Send + Sync + 'static,
        F: for<'a> ReplierFn<'a, M, U, Q, S> + Clone,
        U: Send + 'static,
        Q: Send + 'static,
        S: Send + 'static,
    {
        let sender = Box::new(FilterMapReplierSender::new(
            query_filter_map,
            reply_map,
            replier,
            address.into().0,
        ));
        self.broadcaster.write().unwrap().add(sender);
    }

    /// Broadcasts a query to all connected replier ports.
    pub async fn send(&mut self, arg: T) -> impl Iterator<Item = R> + '_ {
        self.broadcaster
            .write_scratchpad()
            .unwrap()
            .broadcast(arg)
            .await
            .unwrap_or_throw()
    }
}

// Manual clone implementation to cover cases when R is not Clone
impl<T: Clone + Send, R: Send> Clone for Requestor<T, R> {
    fn clone(&self) -> Self {
        Self {
            broadcaster: Clone::clone(&self.broadcaster),
        }
    }
}

impl<T: Clone + Send + 'static, R: Send + 'static> Default for Requestor<T, R> {
    fn default() -> Self {
        Self {
            broadcaster: CachedRwLock::new(QueryBroadcaster::default()),
        }
    }
}

impl<T: Clone + Send + 'static, R: Send + 'static> fmt::Debug for Requestor<T, R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Requestor ({} connected ports)",
            self.broadcaster.read_unsync().len()
        )
    }
}

impl<T: Clone + Send, R: Send> Serialize for Requestor<T, R> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Use serialization to store output copy in the global registry.
        PORT_REG.map(|reg| reg.lock().unwrap().push_back(Box::new(self.clone())));
        serializer.serialize_unit()
    }
}

impl<'de, T: Clone + Send, R: Send> Deserialize<'de> for Requestor<T, R> {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Pop stored value from the registry.
        // If registry is not present, use a default
        Ok(*PORT_REG
            .map(|reg| {
                (reg.lock().unwrap().pop_front().unwrap() as Box<dyn Any>)
                    .downcast()
                    .unwrap()
            })
            .unwrap_or_default())
    }
}

/// A requestor port with exactly one connection.
///
/// A `UniRequestor` port is connected to a replier port, i.e. to an
/// asynchronous model method that returns a value.
pub struct UniRequestor<T: Clone + Send + 'static, R: Send + 'static> {
    sender: Box<dyn Sender<T, R>>,
}

impl<T: Clone + Send + 'static, R: Send + 'static> UniRequestor<T, R> {
    /// Creates a `UniRequestor` port connected to a replier port of the model
    /// specified by the address.
    ///
    /// The replier port must be an asynchronous method of a model of type `M`
    /// returning a value of type `R` and taking as argument a value of type `T`
    /// plus, optionally, a context reference.
    pub fn new<M, F, S>(replier: F, address: impl Into<Address<M>>) -> Self
    where
        M: Model,
        F: for<'a> ReplierFn<'a, M, T, R, S> + Clone,
        S: Send + 'static,
    {
        let sender = Box::new(ReplierSender::new(replier, address.into().0));

        Self { sender }
    }

    /// Creates an auto-converting `UniRequestor` port connected to a replier
    /// port of the model specified by the address.
    ///
    /// Queries and replies are mapped to other types using the closures
    /// provided in argument.
    ///
    /// The replier port must be an asynchronous method of a model of type `M`
    /// returning a value of the type returned by the reply mapping closure and
    /// taking as argument a value of the type returned by the query mapping
    /// closure plus, optionally, a context reference.
    pub fn with_map<M, C, D, F, U, Q, S>(
        query_map: C,
        reply_map: D,
        replier: F,
        address: impl Into<Address<M>>,
    ) -> Self
    where
        M: Model,
        C: Fn(&T) -> U + Send + Sync + 'static,
        D: Fn(Q) -> R + Send + Sync + 'static,
        F: for<'a> ReplierFn<'a, M, U, Q, S> + Clone,
        U: Send + 'static,
        Q: Send + 'static,
        S: Send + 'static,
    {
        let sender = Box::new(MapReplierSender::new(
            query_map,
            reply_map,
            replier,
            address.into().0,
        ));

        Self { sender }
    }

    /// Creates an auto-converting, filtered `UniRequestor` port connected to a
    /// replier port of the model specified by the address.
    ///
    /// Queries and replies are mapped to other types using the closures
    /// provided in argument, or ignored if the query closure returns `None`.
    ///
    /// The replier port must be an asynchronous method of a model of type `M`
    /// returning a value of the type returned by the reply mapping closure and
    /// taking as argument a value of the type returned by the query mapping
    /// closure plus, optionally, a context reference.
    pub fn with_filter_map<M, C, D, F, U, Q, S>(
        query_filter_map: C,
        reply_map: D,
        replier: F,
        address: impl Into<Address<M>>,
    ) -> Self
    where
        M: Model,
        C: Fn(&T) -> Option<U> + Send + Sync + 'static,
        D: Fn(Q) -> R + Send + Sync + 'static,
        F: for<'a> ReplierFn<'a, M, U, Q, S> + Clone,
        U: Send + 'static,
        Q: Send + 'static,
        S: Send + 'static,
    {
        let sender = Box::new(FilterMapReplierSender::new(
            query_filter_map,
            reply_map,
            replier,
            address.into().0,
        ));

        Self { sender }
    }

    /// Sends a query to the connected replier port.
    pub async fn send(&mut self, arg: T) -> Option<R> {
        if let Some(fut) = self.sender.send_owned(arg) {
            let output = fut.await.unwrap_or_throw();

            Some(output)
        } else {
            None
        }
    }
}

// Manual clone implementation to cover cases when R is not Clone
impl<T: Clone + Send, R: Send> Clone for UniRequestor<T, R> {
    fn clone(&self) -> Self {
        Self {
            sender: Clone::clone(&self.sender),
        }
    }
}

impl<T: Clone + Send + 'static, R: Send + 'static> fmt::Debug for UniRequestor<T, R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "UniRequestor")
    }
}

impl<T: Clone + Send, R: Send> Serialize for UniRequestor<T, R> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Use serialization to store output copy in the global registry.
        PORT_REG.map(|reg| reg.lock().unwrap().push_back(Box::new(self.clone())));
        serializer.serialize_unit()
    }
}

impl<'de, T: Clone + Send, R: Send> Deserialize<'de> for UniRequestor<T, R> {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Pop stored value from the registry.
        // Since there is no ease way to create a default
        // a dummy sender is used instead.
        Ok(*PORT_REG
            .map(|reg| {
                (reg.lock().unwrap().pop_front().unwrap() as Box<dyn Any>)
                    .downcast()
                    .unwrap()
            })
            .unwrap_or(Box::new(UniRequestor {
                sender: Box::new(EmptySender),
            })))
    }
}

// TODO validate this
#[derive(Clone)]
struct EmptySender;
impl<T, R> Sender<T, R> for EmptySender {
    fn send(
        &mut self,
        _: &T,
    ) -> Option<sender::RecycledFuture<'_, Result<R, crate::channel::SendError>>> {
        None
    }
    fn send_owned(
        &mut self,
        _: T,
    ) -> Option<sender::RecycledFuture<'_, Result<R, crate::channel::SendError>>> {
        None
    }
}
