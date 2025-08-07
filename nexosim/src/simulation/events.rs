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
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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

// This value has to be lower than the `SchedulableId::Mask`.
const MAX_SOURCE_ID: usize = 1 << (usize::BITS - 1) as usize;

/// A unique, type-safe id for schedulable event sources.
/// `SourceId` is stable between bench runs, provided that the bench layout does
/// not change.
#[derive(Debug, Serialize, Deserialize)]
pub struct SourceId<T>(pub(crate) usize, pub(crate) PhantomData<fn(T)>);

// Manual clone and copy impl. to not enforce bounds on T.
impl<T> Clone for SourceId<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for SourceId<T> {}

/// Type erased `SourceId` variant.
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

/// Scheduler event source registry.
/// Only events present in the registry can be scheduled for a future execution
/// and put on the queue.
///
/// Event registration has to take place before simulation is started / resumed.
/// Therefore the `add` method should only be accessible from `SimInit` or
/// `BuildContext` instances.
#[derive(Default, Debug)]
pub(crate) struct SchedulerSourceRegistry(Vec<Box<dyn SchedulerEventSource>>);
impl SchedulerSourceRegistry {
    pub(crate) fn add<T>(&mut self, source: impl TypedSchedulerSource<T>) -> SourceId<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        assert!(self.0.len() < MAX_SOURCE_ID);
        let source_id = SourceId(self.0.len(), PhantomData);
        self.0.push(Box::new(source));
        source_id
    }
    pub(crate) fn get(&self, source_id: &SourceIdErased) -> Option<&dyn SchedulerEventSource> {
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

/// A helper trait ensuring type safety of the registered event sources.
///
/// It is necessary for the registered sources to implement this trait in order
/// to provide a type safe registration interface.
pub(crate) trait TypedSchedulerSource<T>: SchedulerEventSource {}

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

impl<M, F, S, T> TypedSchedulerSource<T> for InputSource<M, F, S, T>
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
        serialize_event_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_event_arg::<T>(arg)
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

impl<T> TypedSchedulerSource<T> for EventSource<T> where
    T: Serialize + DeserializeOwned + Clone + Send + 'static
{
}

impl<T> SchedulerEventSource for EventSource<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Result<Vec<u8>, ExecutionError> {
        serialize_event_arg::<T>(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Result<Box<dyn Any + Send>, ExecutionError> {
        deserialize_event_arg::<T>(arg)
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

impl<T> TypedSchedulerSource<T> for Arc<EventSource<T>> where
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

/// A possibly periodic, possibly cancellable event that can be scheduled on
/// the event queue.
#[derive(Debug)]
pub(crate) struct Event {
    pub source_id: SourceIdErased,
    pub arg: Box<dyn Any + Send>,
    pub period: Option<Duration>,
    pub key: Option<EventKey>,
}
impl Event {
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
                .map_err(|_| ExecutionError::RestoreError("ScheduledEvent".to_string()))?
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

/// Local helper struct organizing event data for serialization.
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
fn deserialize_event_arg<T: DeserializeOwned + Send + 'static>(
    arg: &[u8],
) -> Result<Box<dyn Any + Send>, ExecutionError> {
    Ok(Box::new(
        bincode::serde::borrow_decode_from_slice::<T, _>(arg, serialization_config())
            .map_err(|_| ExecutionError::RestoreError(format!("Event arg ({})", type_name::<T>())))?
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
