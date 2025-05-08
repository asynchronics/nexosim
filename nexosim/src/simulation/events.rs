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

use recycle_box::{coerce_box, RecycleBox};
use serde::de::Visitor;
use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize};

use crate::channel::Sender;
use crate::macros::scoped_thread_local::scoped_thread_local;
use crate::model::Model;
use crate::ports::{EventSource, InputFn};
use crate::simulation::Address;
use crate::util::serialization::serialization_config;
use crate::util::unwrap_or_throw::UnwrapOrThrow;

use super::ExecutionError;

scoped_thread_local!(pub(crate) static EVENT_KEY_REG: EventKeyReg);
pub(crate) type EventKeyReg = Arc<Mutex<HashMap<usize, Arc<AtomicBool>>>>;

#[derive(Debug, Serialize, Deserialize)]
pub struct SourceId<T>(pub(crate) usize, pub(crate) PhantomData<T>);

// Manual clone and copy impl. to not enforce bounds on T.
impl<T> Clone for SourceId<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for SourceId<T> {}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SourceIdErased(pub(crate) usize);

impl<T> From<SourceId<T>> for SourceIdErased {
    fn from(value: SourceId<T>) -> Self {
        Self(value.0)
    }
}

impl<T> From<&SourceId<T>> for SourceIdErased {
    fn from(value: &SourceId<T>) -> Self {
        Self(value.0)
    }
}

#[derive(Default, Debug)]
pub(crate) struct SchedulerSourceRegistry(Vec<Box<dyn SchedulerEventSource>>);
impl SchedulerSourceRegistry {
    pub(crate) fn add<T>(&mut self, source: impl TypedEventSource<T>) -> SourceId<T>
    where
        for<'de> T: Serialize + Deserialize<'de> + Clone + Send + 'static,
    {
        let source_id = SourceId(self.0.len(), PhantomData);
        self.0.push(Box::new(source));
        source_id
    }
    pub(crate) fn get(&self, source_id: &SourceIdErased) -> Option<&dyn SchedulerEventSource> {
        self.0.get(source_id.0).map(|s| s.as_ref())
    }
}

pub(crate) trait SchedulerEventSource: std::fmt::Debug + Send + 'static {
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError>;
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError>;
    fn into_future(
        &self,
        arg: &dyn Any,
        event_key: Option<EventKey>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub(crate) trait TypedEventSource<T>: SchedulerEventSource {}

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
    pub fn new(func: F, address: impl Into<Address<M>>) -> Self {
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
    for<'de> T: Serialize + Deserialize<'de> + Clone + Send + 'static,
{
}
impl<M, F, S, T> SchedulerEventSource for InputSource<M, F, S, T>
where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
    S: Send + Sync + 'static,
    for<'de> T: Serialize + Deserialize<'de> + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        serialize_event_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_event_arg::<T>(arg)
    }
    fn into_future(
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
                    move |model: &mut M, scheduler, recycle_box: RecycleBox<()>| {
                        let fut = async {
                            match event_key {
                                Some(key) if key.is_cancelled() => (),
                                _ => func.call(model, arg, scheduler).await,
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
    for<'de> T: Serialize + Deserialize<'de> + Clone + Send + 'static
{
}

impl<T> SchedulerEventSource for EventSource<T>
where
    for<'de> T: Serialize + Deserialize<'de> + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        serialize_event_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_event_arg::<T>(arg)
    }
    fn into_future(
        &self,
        arg: &dyn Any,
        _: Option<EventKey>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(EventSource::into_future(
            self,
            arg.downcast_ref::<T>().unwrap().clone(),
        ))
    }
}

impl<T> TypedEventSource<T> for Arc<EventSource<T>> where
    for<'de> T: Serialize + Deserialize<'de> + Clone + Send + 'static
{
}
impl<T> SchedulerEventSource for Arc<EventSource<T>>
where
    for<'de> T: Serialize + Deserialize<'de> + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        self.as_ref().serialize_arg(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        self.as_ref().deserialize_arg(arg)
    }
    fn into_future(
        &self,
        arg: &dyn Any,
        event_key: Option<EventKey>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let inner: &dyn SchedulerEventSource = self.as_ref();
        inner.into_future(arg, event_key)
    }
}

/// Struct representing a single scheduled event on the queue.
#[derive(Debug)]
pub(crate) struct ScheduledEvent {
    pub source_id: SourceIdErased,
    pub arg: Box<dyn Any + Send>,
    pub period: Option<Duration>,
    pub key: Option<EventKey>,
}
impl ScheduledEvent {
    pub(crate) fn new<T: Send + 'static>(source_id: &SourceId<T>, arg: T) -> Self {
        Self {
            source_id: source_id.into(),
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

    pub(crate) fn serialize(
        &self,
        registry: &SchedulerSourceRegistry,
    ) -> Result<Vec<u8>, ExecutionError> {
        let source = registry
            .get(&self.source_id)
            .ok_or(ExecutionError::SaveError(format!(
                "ScheduledEvent({}) (id not found)",
                self.source_id.0
            )))?;
        let arg = source.serialize_arg(&*self.arg)?;

        bincode::serde::encode_to_vec(
            SerializableEvent {
                source_id: self.source_id,
                arg,
                period: self.period,
                key: self.key.clone(),
            },
            serialization_config(),
        )
        .map_err(|_| ExecutionError::SaveError(format!("ScheduledEvent({})", self.source_id.0)))
    }

    pub(crate) fn deserialize(
        data: &[u8],
        registry: &SchedulerSourceRegistry,
    ) -> Result<Self, ExecutionError> {
        let mut event: SerializableEvent =
            bincode::serde::decode_from_slice(data, serialization_config())
                .map_err(|_| ExecutionError::RestoreError(format!("ScheduledEvent")))?
                .0;

        let source = registry
            .get(&event.source_id)
            .ok_or(ExecutionError::RestoreError(format!(
                "ScheduledEvent({}) (id not found)",
                event.source_id.0,
            )))?;
        let arg = source.deserialize_arg(&event.arg)?;

        Ok(Self {
            source_id: event.source_id,
            arg,
            period: event.period,
            key: event.key.take(),
        })
    }
}

/// Local helper struct organizing data for serialization
#[derive(Serialize, Deserialize)]
struct SerializableEvent {
    source_id: SourceIdErased,
    arg: Vec<u8>,
    period: Option<Duration>,
    key: Option<EventKey>,
}

fn serialize_event_arg<T: Serialize + Send + 'static>(
    arg: &dyn Any,
) -> Result<Vec<u8>, ExecutionError> {
    let value = arg
        .downcast_ref::<T>()
        .ok_or(ExecutionError::SaveError(format!(
            "Event arg ({})",
            type_name::<T>()
        )))?;
    bincode::serde::encode_to_vec(value, serialization_config())
        .map_err(|_| ExecutionError::SaveError(format!("Event arg: {}", type_name::<T>())))
}
fn deserialize_event_arg<T: for<'de> Deserialize<'de> + Send + 'static>(
    arg: &[u8],
) -> Result<Box<dyn Any + Send>, ExecutionError> {
    Ok(Box::new(
        bincode::serde::borrow_decode_from_slice::<T, _>(arg, serialization_config())
            .map_err(|_| ExecutionError::RestoreError(format!("Event arg ({})", type_name::<T>())))?
            .0,
    ))
}

/// Managed handle to a scheduled action.
///
/// An `AutoEventKey` is a managed handle to a scheduled action that cancels
/// its associated action on drop.
#[derive(Debug)]
#[must_use = "dropping this key immediately cancels the associated action"]
pub struct AutoEventKey {
    is_cancelled: Arc<AtomicBool>,
}

impl Drop for AutoEventKey {
    fn drop(&mut self) {
        self.is_cancelled.store(true, Ordering::Relaxed);
    }
}

/// Handle to a scheduled action.
///
/// An `ActionKey` can be used to cancel a scheduled action.
#[derive(Clone, Debug)]
#[must_use = "prefer unkeyed scheduling methods if the action is never cancelled"]
pub struct EventKey {
    is_cancelled: Arc<AtomicBool>,
}

impl EventKey {
    /// Creates a key for a pending action.
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
