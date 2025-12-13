use std::any::{type_name, Any};
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_channel::oneshot;
use recycle_box::{coerce_box, RecycleBox};
use serde::de::Visitor;
use serde::ser::SerializeTuple;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::channel::Sender;
use crate::macros::scoped_thread_local::scoped_thread_local;
use crate::model::Model;
use crate::ports::{EventSource, InputFn, QuerySource, ReplyIterator};
use crate::simulation::{Address, RestoreError, SaveError};
use crate::util::serialization::serialization_config;
use crate::util::unwrap_or_throw::UnwrapOrThrow;

use super::ExecutionError;

scoped_thread_local!(pub(crate) static EVENT_KEY_REG: EventKeyReg);
pub(crate) type EventKeyReg = Arc<Mutex<HashMap<usize, Arc<AtomicBool>>>>;

// This value has to be lower than the `SchedulableId::Mask`.
const MAX_SOURCE_ID: usize = 1 << (usize::BITS - 1) as usize;

/// A unique, type-safe id for schedulable event sources.
/// `SourceId` is stable between bench runs, provided that the bench layout does
/// not change.
#[derive(Debug, Serialize, Deserialize)]
pub struct EventId<T>(pub(crate) usize, pub(crate) PhantomData<fn(T)>);

// Manual clone and copy impl. to not enforce bounds on T.
impl<T> Clone for EventId<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for EventId<T> {}

/// Type erased `EventId` variant.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct EventIdErased(pub(crate) usize);

impl<T> From<EventId<T>> for EventIdErased {
    fn from(value: EventId<T>) -> Self {
        Self(value.0)
    }
}

impl<T> From<&EventId<T>> for EventIdErased {
    fn from(value: &EventId<T>) -> Self {
        Self(value.0)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryId<T, R>(pub(crate) usize, pub(crate) PhantomData<fn(T, R)>);

// Manual clone and copy impl. to not enforce bounds on T.
impl<T, R> Clone for QueryId<T, R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, R> Copy for QueryId<T, R> {}

/// Type erased `QueryId` variant.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct QueryIdErased(pub(crate) usize);

impl<T, R> From<QueryId<T, R>> for QueryIdErased {
    fn from(value: QueryId<T, R>) -> Self {
        Self(value.0)
    }
}

impl<T, R> From<&QueryId<T, R>> for QueryIdErased {
    fn from(value: &QueryId<T, R>) -> Self {
        Self(value.0)
    }
}

#[derive(Default, Debug)]
pub(crate) struct SchedulerRegistry {
    pub(crate) event_registry: SchedulerEventRegistry,
    pub(crate) query_registry: SchedulerQueryRegistry,
}

/// Scheduler event source registry.
/// Only events present in the registry can be scheduled for a future execution
/// and put on the queue.
///
/// Event registration has to take place before simulation is started / resumed.
/// Therefore the `add` method should only be accessible from `SimInit` or
/// `BuildContext` instances.
#[derive(Default, Debug)]
pub(crate) struct SchedulerEventRegistry(Vec<Box<dyn SchedulerEventSource>>);
impl SchedulerEventRegistry {
    pub(crate) fn add<T>(&mut self, source: impl TypedEventSource<T>) -> EventId<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        assert!(self.0.len() < MAX_SOURCE_ID);
        let event_id = EventId(self.0.len(), PhantomData);
        self.0.push(Box::new(source));
        event_id
    }
    pub(crate) fn get(&self, source_id: &EventIdErased) -> Option<&dyn SchedulerEventSource> {
        self.0.get(source_id.0).map(|s| s.as_ref())
    }
}

#[derive(Default, Debug)]
pub(crate) struct SchedulerQueryRegistry(Vec<Box<dyn SchedulerQuerySource>>);
impl SchedulerQueryRegistry {
    pub(crate) fn add<T, R>(&mut self, source: impl TypedQuerySource<T, R>) -> QueryId<T, R>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        assert!(self.0.len() < MAX_SOURCE_ID);
        let query_id = QueryId(self.0.len(), PhantomData);
        self.0.push(Box::new(source));
        query_id
    }
    pub(crate) fn get(&self, source_id: &QueryIdErased) -> Option<&dyn SchedulerQuerySource> {
        self.0.get(source_id.0).map(|s| s.as_ref())
    }
}

/// Internal SchedulerSourceRegistry entry interface.
pub(crate) trait SchedulerEventSource: std::fmt::Debug + Send + 'static {
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError>;
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError>;
    fn event_future(
        &self,
        arg: &dyn Any,
        event_key: Option<EventKey>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub(crate) trait SchedulerQuerySource: std::fmt::Debug + Send + 'static {
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError>;
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError>;
    fn query_future(
        &self,
        arg: &dyn Any,
        replier: Option<Box<dyn Any + Send>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// A helper trait ensuring type safety of the registered event sources.
///
/// It is necessary for the registered sources to implement this trait in order
/// to provide a type safe registration interface.
pub(crate) trait TypedEventSource<T>: SchedulerEventSource {}
pub(crate) trait TypedQuerySource<T, R>: SchedulerQuerySource {}

/// A specialized event source struct used to register models' input methods.
///
/// Unlike the EventSource struct it does not allow for multiple senders and it
/// is bound to a specific model's input only.
pub(crate) struct InputSource<M, F, S, T>
where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
    S: Send + Sync + 'static,
    T: Clone + Send + 'static,
{
    sender: Sender<M>,
    func: F,
    _phantom: PhantomData<(T, S)>,
}
impl<M, F, S, T> InputSource<M, F, S, T>
where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
    S: Send + Sync + 'static,
    T: Clone + Send + 'static,
{
    pub(crate) fn new(func: F, address: impl Into<Address<M>>) -> Self {
        let sender = address.into().0;

        Self {
            sender,
            func,
            _phantom: PhantomData,
        }
    }
}

impl<M, F, S, T> std::fmt::Debug for InputSource<M, F, S, T>
where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
    S: Send + Sync + 'static,
    T: Clone + Send + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Input source, T: {}", type_name::<T>())
    }
}

impl<M, F, S, T> TypedEventSource<T> for InputSource<M, F, S, T>
where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
    S: Send + Sync + 'static,
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
}
impl<M, F, S, T> SchedulerEventSource for InputSource<M, F, S, T>
where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
    S: Send + Sync + 'static,
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        serialize_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_arg::<T>(arg)
    }
    fn event_future(
        &self,
        arg: &dyn Any,
        event_key: Option<EventKey>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let arg = arg.downcast_ref::<T>().unwrap().clone();
        let func = self.func.clone();
        let sender = self.sender.clone();

        let fut = async move {
            sender
                .send(
                    move |model: &mut M, scheduler, env, recycle_box: RecycleBox<()>| {
                        let fut = async {
                            match event_key {
                                Some(key) if key.is_cancelled() => (),
                                _ => func.call(model, arg, scheduler, env).await,
                            }
                        };

                        coerce_box!(RecycleBox::recycle(recycle_box, fut))
                    },
                )
                .await
                .unwrap_or_throw();
        };

        Box::pin(fut)
    }
}

impl<T> TypedEventSource<T> for EventSource<T> where
    T: Serialize + DeserializeOwned + Clone + Send + 'static
{
}

impl<T> SchedulerEventSource for EventSource<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        serialize_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_arg::<T>(arg)
    }
    fn event_future(
        &self,
        arg: &dyn Any,
        _: Option<EventKey>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(EventSource::event_future(
            self,
            arg.downcast_ref::<T>().unwrap().clone(),
        ))
    }
}

impl<T> TypedEventSource<T> for Arc<EventSource<T>> where
    T: Serialize + DeserializeOwned + Clone + Send + 'static
{
}
impl<T> SchedulerEventSource for Arc<EventSource<T>>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        self.as_ref().serialize_arg(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        self.as_ref().deserialize_arg(arg)
    }
    fn event_future(
        &self,
        arg: &dyn Any,
        event_key: Option<EventKey>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let inner: &dyn SchedulerEventSource = self.as_ref();
        inner.event_future(arg, event_key)
    }
}

impl<T, R> TypedQuerySource<T, R> for QuerySource<T, R>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Send + 'static,
{
}
impl<T, R> SchedulerQuerySource for QuerySource<T, R>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        serialize_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_arg::<T>(arg)
    }
    fn query_future(
        &self,
        arg: &dyn Any,
        replier: Option<Box<dyn Any + Send>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let replier = replier.map(|r| *r.downcast::<ReplyWriter<R>>().unwrap());
        Box::pin(QuerySource::query_future(
            self,
            arg.downcast_ref::<T>().unwrap().clone(),
            replier,
        ))
    }
}

impl<T, R> TypedQuerySource<T, R> for Arc<QuerySource<T, R>>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Send + 'static,
{
}
impl<T, R> SchedulerQuerySource for Arc<QuerySource<T, R>>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    R: Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        serialize_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_arg::<T>(arg)
    }
    fn query_future(
        &self,
        arg: &dyn Any,
        replier: Option<Box<dyn Any + Send>>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let inner: &dyn SchedulerQuerySource = self.as_ref();
        inner.query_future(arg, replier)
    }
}

pub(crate) enum QueueItem {
    Event(Event),
    Query(Query),
}
impl QueueItem {
    pub(crate) fn serialize(
        &self,
        registry: &SchedulerRegistry,
    ) -> Result<Vec<u8>, ExecutionError> {
        let serializable = match self {
            QueueItem::Event(event) => {
                SerializableQueueItem::Event(event.to_serializable(&registry.event_registry)?)
            }
            QueueItem::Query(query) => {
                SerializableQueueItem::Query(query.to_serializable(&registry.query_registry)?)
            }
        };

        bincode::serde::encode_to_vec(serializable, serialization_config()).map_err(|e| {
            match self {
                QueueItem::Event(event) => SaveError::EventSerializationError {
                    event_id: event.event_id.0,
                    cause: Box::new(e),
                },
                QueueItem::Query(query) => SaveError::QuerySerializationError {
                    query_id: query.query_id.0,
                    cause: Box::new(e),
                },
            }
            .into()
        })
    }

    pub(crate) fn deserialize(
        data: &[u8],
        registry: &SchedulerRegistry,
    ) -> Result<Self, ExecutionError> {
        let item: SerializableQueueItem =
            bincode::serde::decode_from_slice(data, serialization_config())
                .map_err(|e| RestoreError::QueueItemDeserializationError { cause: Box::new(e) })?
                .0;

        Ok(match item {
            SerializableQueueItem::Event(event) => {
                QueueItem::Event(Event::from_serializable(event, &registry.event_registry)?)
            }
            SerializableQueueItem::Query(query) => {
                QueueItem::Query(Query::from_serializable(query, &registry.query_registry)?)
            }
        })
    }
}

/// A possibly periodic, possibly cancellable event that can be scheduled on
/// the event queue.
// #[derive(Debug)]
pub(crate) struct Event {
    pub event_id: EventIdErased,
    pub arg: Box<dyn Any + Send>,
    pub period: Option<Duration>,
    pub key: Option<EventKey>,
}
impl Event {
    pub(crate) fn new<T: Send + 'static>(event_id: &EventId<T>, arg: T) -> Self {
        Self {
            event_id: event_id.into(),
            arg: Box::new(arg),
            period: None,
            key: None,
        }
    }

    pub(crate) fn with_period(mut self, period: Duration) -> Self {
        self.period = Some(period);
        self
    }

    pub(crate) fn with_key(mut self, key: EventKey) -> Self {
        self.key = Some(key);
        self
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.key.as_ref().map(|k| k.is_cancelled()).unwrap_or(false)
    }

    pub(crate) fn to_serializable(
        &self,
        registry: &SchedulerEventRegistry,
    ) -> Result<SerializableEvent, ExecutionError> {
        let source = registry
            .get(&self.event_id)
            .ok_or(SaveError::EventNotFound {
                event_id: self.event_id.0,
            })?;
        let arg = source.serialize_arg(&*self.arg)?;
        Ok(SerializableEvent {
            event_id: self.event_id,
            arg,
            period: self.period,
            key: self.key.clone(),
        })
    }

    pub(crate) fn from_serializable(
        mut event: SerializableEvent,
        registry: &SchedulerEventRegistry,
    ) -> Result<Self, ExecutionError> {
        let source = registry
            .get(&event.event_id)
            .ok_or(RestoreError::EventNotFound {
                event_id: event.event_id.0,
            })?;
        let arg = source.deserialize_arg(&event.arg)?;

        Ok(Self {
            event_id: event.event_id,
            arg,
            period: event.period,
            key: event.key.take(),
        })
    }
}

pub(crate) struct Query {
    pub query_id: QueryIdErased,
    pub arg: Box<dyn Any + Send>,
    pub replier: Option<Box<dyn Any + Send>>,
}
impl Query {
    pub(crate) fn new<T: Send + 'static, R: Send + 'static>(
        query_id: &QueryId<T, R>,
        arg: T,
        replier: ReplyWriter<R>,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            arg: Box::new(arg),
            replier: Some(Box::new(replier)),
        }
    }
    pub(crate) fn to_serializable(
        &self,
        registry: &SchedulerQueryRegistry,
    ) -> Result<SerializableQuery, ExecutionError> {
        let source = registry
            .get(&self.query_id)
            .ok_or(SaveError::QueryNotFound {
                query_id: self.query_id.0,
            })?;
        let arg = source.serialize_arg(&*self.arg)?;
        Ok(SerializableQuery {
            query_id: self.query_id,
            arg,
        })
    }

    pub(crate) fn from_serializable(
        query: SerializableQuery,
        registry: &SchedulerQueryRegistry,
    ) -> Result<Self, ExecutionError> {
        let source = registry
            .get(&query.query_id)
            .ok_or(RestoreError::QueryNotFound {
                query_id: query.query_id.0,
            })?;
        let arg = source.deserialize_arg(&query.arg)?;

        Ok(Self {
            query_id: query.query_id,
            arg,
            replier: None,
        })
    }
}

// TODO placeholder
#[derive(Debug)]
pub struct ReplyReader<R>(oneshot::Receiver<ReplyIterator<R>>);
impl<R: Send + 'static> ReplyReader<R> {
    pub fn try_read(&mut self) -> Option<impl Iterator<Item = R>> {
        self.0.try_recv().ok()?
    }

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

#[derive(Serialize, Deserialize)]
enum SerializableQueueItem {
    Event(SerializableEvent),
    Query(SerializableQuery),
}

/// Local helper struct organizing event data for serialization.
#[derive(Serialize, Deserialize)]
struct SerializableEvent {
    event_id: EventIdErased,
    arg: Vec<u8>,
    period: Option<Duration>,
    key: Option<EventKey>,
}

/// Local helper struct organizing event data for serialization.
#[derive(Serialize, Deserialize)]
struct SerializableQuery {
    query_id: QueryIdErased,
    arg: Vec<u8>,
}

fn serialize_arg<T: Serialize + Send + 'static>(arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
    let value = arg
        .downcast_ref::<T>()
        .ok_or(SaveError::ArgumentTypeMismatch {
            type_name: type_name::<T>(),
        })?;
    bincode::serde::encode_to_vec(value, serialization_config()).map_err(|e| {
        SaveError::ArgumentSerializationError {
            type_name: type_name::<T>(),
            cause: Box::new(e),
        }
        .into()
    })
}
fn deserialize_arg<T: DeserializeOwned + Send + 'static>(
    arg: &[u8],
) -> Result<Box<dyn Any + Send>, ExecutionError> {
    Ok(Box::new(
        bincode::serde::borrow_decode_from_slice::<T, _>(arg, serialization_config())
            .map_err(|e| RestoreError::ArgumentDeserializationError {
                type_name: type_name::<T>(),
                cause: Box::new(e),
            })?
            .0,
    ))
}

/// Managed handle to a scheduled event.
///
/// An `AutoEventKey` is a managed handle to a scheduled action that cancels
/// its associated action on drop.
#[derive(Debug, Deserialize)]
#[must_use = "dropping this key immediately cancels the associated action"]
#[serde(from = "EventKey")]
pub struct AutoEventKey {
    is_cancelled: Arc<AtomicBool>,
}

impl Drop for AutoEventKey {
    fn drop(&mut self) {
        self.is_cancelled.store(true, Ordering::Relaxed);
    }
}

// Required for auto deserialization
impl From<EventKey> for AutoEventKey {
    fn from(value: EventKey) -> Self {
        value.into_auto()
    }
}

// Auto serialization via serde(into) seems not possible because of the drop
// implementation.
impl Serialize for AutoEventKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let key = EventKey {
            is_cancelled: self.is_cancelled.clone(),
        };
        key.serialize(serializer)
    }
}

/// Handle to a scheduled event.
///
/// An `EventKey` can be used to cancel a scheduled action.
#[derive(Clone, Debug)]
#[must_use = "prefer unkeyed scheduling methods if the action is never cancelled"]
pub struct EventKey {
    is_cancelled: Arc<AtomicBool>,
}

impl EventKey {
    /// Creates a key for a pending event.
    pub(crate) fn new() -> Self {
        Self {
            is_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a key from an existing atomic bool
    fn restore(is_cancelled: Arc<AtomicBool>) -> Self {
        Self { is_cancelled }
    }

    /// Checks whether the action was cancelled.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Relaxed)
    }

    /// Cancels the associated action.
    pub fn cancel(self) {
        self.is_cancelled.store(true, Ordering::Relaxed);
    }

    /// Converts action key to a managed key.
    pub fn into_auto(self) -> AutoEventKey {
        AutoEventKey {
            is_cancelled: self.is_cancelled,
        }
    }
}

impl PartialEq for EventKey {
    /// Implements equality by considering clones to be equivalent, rather than
    /// keys with the same `is_cancelled` value.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.is_cancelled, &other.is_cancelled)
    }
}

impl Eq for EventKey {}

impl Hash for EventKey {
    /// Implements `Hash`` by considering clones to be equivalent, rather than
    /// keys with the same `is_cancelled` value.
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        ptr::hash(&*self.is_cancelled, state)
    }
}

impl Serialize for EventKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut tuple = serializer.serialize_tuple(2)?;
        tuple.serialize_element(&self.is_cancelled.load(Ordering::Relaxed))?;
        tuple.serialize_element(&Arc::as_ptr(&self.is_cancelled).addr())?;
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for EventKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KeyVisitor;
        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = EventKey;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("tuple")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let value = seq
                    .next_element()?
                    .ok_or(serde::de::Error::custom("Expected bool"))?;
                let addr: usize = seq
                    .next_element()?
                    .ok_or(serde::de::Error::custom("Expected usize"))?;
                Ok(EVENT_KEY_REG
                    .map(|reg| {
                        let mut reg = reg.lock().unwrap();
                        let target = reg.entry(addr).or_insert(Arc::new(value));
                        EventKey::restore(target.clone())
                    })
                    .unwrap_or(EventKey::new()))
            }
        }

        deserializer.deserialize_tuple_struct("EventKey", 2, KeyVisitor)
    }
}

/// A one-shot action that can be processed immediately.
///
/// `Actions` can be created from an [`EventSource`]
/// or [`QuerySource`](crate::ports::QuerySource). They can be used to process
/// events and requests immediately with
/// [`Simulation::process_action`](crate::simulation::Simulation::process_action).
pub struct Action {
    future: Pin<Box<dyn Future<Output = ()> + Send>>,
}
impl Action {
    pub(crate) fn new(future: Pin<Box<dyn Future<Output = ()> + Send>>) -> Self {
        Self { future }
    }
    pub(crate) fn consume(self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        self.future
    }
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Action").finish_non_exhaustive()
    }
}

mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn serialize_auto_key() {
        let event_key = EventKey::new();
        let auto = event_key.clone().into_auto();

        let state = bincode::serde::encode_to_vec((&auto, &event_key), bincode::config::standard())
            .unwrap();

        // Check if serializing does not auto cancel
        assert!(!event_key.is_cancelled());

        // Check connection after deserialization
        let event_key_reg = Arc::new(Mutex::new(HashMap::new()));
        let (auto, event_key): (AutoEventKey, EventKey) = EVENT_KEY_REG
            .set::<_, Result<(AutoEventKey, EventKey), ExecutionError>>(&event_key_reg, || {
                Ok(
                    bincode::serde::decode_from_slice(&state, bincode::config::standard())
                        .unwrap()
                        .0,
                )
            })
            .unwrap();

        assert!(!event_key.is_cancelled());
        drop(auto);
        assert!(event_key.is_cancelled());
    }
}
