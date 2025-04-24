use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bincode;
use serde::de::{DeserializeOwned, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Deserializer, Serialize};

use crate::macros::scoped_thread_local::scoped_thread_local;
use crate::ports::EventSource;

scoped_thread_local!(pub(crate) static ACTION_KEYS: ActionKeyReg);

pub(crate) type ActionKeyReg = Arc<Mutex<HashMap<usize, Arc<AtomicBool>>>>;

// Typed SourceId allows for compile time argument validation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SourceId<T>(pub(crate) usize, pub(crate) PhantomData<T>);

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SourceIdErased(pub(crate) usize);

impl<T> From<SourceId<T>> for SourceIdErased {
    fn from(value: SourceId<T>) -> Self {
        Self(value.0)
    }
}

#[derive(Default)]
pub(crate) struct SchedulerSourceRegistry(Vec<Box<dyn SchedulerEventSource>>);
impl SchedulerSourceRegistry {
    pub(crate) fn add<T>(&mut self, source: EventSource<T>) -> SourceId<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        let source_id = SourceId(self.0.len(), PhantomData);
        self.0.push(Box::new(source));
        source_id
    }
    pub(crate) fn add_erased(&mut self, source: Box<dyn SchedulerEventSource>) -> SourceIdErased {
        let source_id = SourceIdErased(self.0.len());
        self.0.push(source);
        source_id
    }
    pub(crate) fn get(&self, source_id: &SourceIdErased) -> Option<&dyn SchedulerEventSource> {
        self.0.get(source_id.0).map(|s| s.as_ref())
    }
}

pub(crate) trait SchedulerEventSource: std::fmt::Debug + Send + Sync + 'static {
    fn serialize_arg(&self, arg: &dyn Any) -> Vec<u8>;
    fn deserialize_arg(&self, arg: &[u8]) -> Box<dyn Any + Send>;
    fn into_future(&self, arg: &dyn Any) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

impl<T> SchedulerEventSource for EventSource<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn deserialize_arg(&self, serialized_arg: &[u8]) -> Box<dyn Any + Send> {
        // TODO error
        let arg: T = bincode::serde::decode_from_slice(
            serialized_arg,
            crate::util::serialization::get_serialization_config(),
        )
        .expect(&format!(
            "Argument deserialization failed. Cannot interpret {:?} as {}",
            serialized_arg,
            std::any::type_name::<T>()
        ))
        .0;
        Box::new(arg)
    }
    fn serialize_arg(&self, arg: &dyn Any) -> Vec<u8> {
        // TODO unwrap
        let value = arg.downcast_ref::<T>().unwrap();
        bincode::serde::encode_to_vec(
            value,
            crate::util::serialization::get_serialization_config(),
        )
        .unwrap()
    }
    fn into_future(&self, arg: &dyn Any) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(EventSource::into_future(
            self,
            // TODO error handling
            arg.downcast_ref::<T>().unwrap().clone(),
        ))
    }
}
impl<T> SchedulerEventSource for Arc<EventSource<T>>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn serialize_arg(&self, arg: &dyn Any) -> Vec<u8> {
        self.as_ref().serialize_arg(arg)
    }
    fn deserialize_arg(&self, arg: &[u8]) -> Box<dyn Any + Send> {
        self.as_ref().deserialize_arg(arg)
    }
    fn into_future(&self, arg: &dyn Any) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let inner: &dyn SchedulerEventSource = self.as_ref();
        inner.into_future(arg)
    }
}

#[derive(Debug)]
pub(crate) struct ScheduledEvent {
    pub source_id: SourceIdErased,
    pub arg: Box<dyn Any + Send>,
    pub period: Option<Duration>,
    pub action_key: Option<ActionKey>,
}
impl ScheduledEvent {
    pub(crate) fn new<T>(source_id: SourceId<T>, arg: Box<dyn Any + Send>) -> Self {
        Self {
            source_id: source_id.into(),
            arg,
            period: None,
            action_key: None,
        }
    }
    pub(crate) fn new_erased(source_id: SourceIdErased, arg: Box<dyn Any + Send>) -> Self {
        Self {
            source_id,
            arg,
            period: None,
            action_key: None,
        }
    }
    pub(crate) fn with_period(mut self, period: Duration) -> Self {
        self.period = Some(period);
        self
    }
    pub(crate) fn with_key(mut self, action_key: ActionKey) -> Self {
        self.action_key = Some(action_key);
        self
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        match &self.action_key {
            Some(key) => key.is_cancelled(),
            None => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SerializableEvent {
    source_id: SourceIdErased,
    arg: Vec<u8>,
    period: Option<Duration>,
    action_key: Option<ActionKey>,
}
impl SerializableEvent {
    pub fn from_scheduled_event(
        event: &ScheduledEvent,
        registry: &SchedulerSourceRegistry,
    ) -> Self {
        // TODO error handling
        let source = registry.get(&event.source_id).unwrap();
        let arg = source.serialize_arg(&*event.arg);
        Self {
            source_id: event.source_id,
            arg,
            period: event.period,
            action_key: event.action_key.clone(),
        }
    }
    pub fn to_scheduled_event(&self, registry: &SchedulerSourceRegistry) -> ScheduledEvent {
        // TODO error handling
        let source = registry.get(&self.source_id).unwrap();
        let arg = source.deserialize_arg(&self.arg);
        ScheduledEvent {
            source_id: self.source_id,
            arg,
            period: self.period,
            action_key: self.action_key.clone(),
        }
    }
}

/// Managed handle to a scheduled action.
///
/// An `AutoActionKey` is a managed handle to a scheduled action that cancels
/// its associated action on drop.
#[derive(Debug, Serialize, Deserialize)]
#[must_use = "dropping this key immediately cancels the associated action"]
pub struct AutoActionKey {
    inner: ActionKey,
}

impl Drop for AutoActionKey {
    fn drop(&mut self) {
        self.inner.is_cancelled.store(true, Ordering::Relaxed);
    }
}

/// Handle to a scheduled action.
///
/// An `ActionKey` can be used to cancel a scheduled action.
#[derive(Clone, Debug)]
#[must_use = "prefer unkeyed scheduling methods if the action is never cancelled"]
pub struct ActionKey {
    is_cancelled: Arc<AtomicBool>,
}

impl ActionKey {
    /// Creates a key for a pending action.
    pub(crate) fn new() -> Self {
        Self {
            is_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

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
    pub fn into_auto(self) -> AutoActionKey {
        AutoActionKey { inner: self }
    }
}

impl PartialEq for ActionKey {
    /// Implements equality by considering clones to be equivalent, rather than
    /// keys with the same `is_cancelled` value.
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(&*self.is_cancelled, &*other.is_cancelled)
    }
}

impl Eq for ActionKey {}

impl Hash for ActionKey {
    /// Implements `Hash`` by considering clones to be equivalent, rather than
    /// keys with the same `is_cancelled` value.
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        ptr::hash(&*self.is_cancelled, state)
    }
}

impl Serialize for ActionKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&self.is_cancelled.load(Ordering::Relaxed))?;
        tup.serialize_element(&Arc::as_ptr(&self.is_cancelled).addr())?;
        tup.end()
    }
}
impl<'de> Deserialize<'de> for ActionKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ActionKeyVisitor;
        impl<'de> Visitor<'de> for ActionKeyVisitor {
            type Value = ActionKey;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("tuple")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let value = seq.next_element()?.unwrap();
                let addr: usize = seq.next_element()?.unwrap();
                ACTION_KEYS
                    .map(|keys| {
                        let mut reg = keys.lock().unwrap();
                        let target = reg.entry(addr).or_insert(Arc::new(value));
                        Ok(ActionKey::restore(target.clone()))
                    })
                    .unwrap()
            }
        }

        deserializer.deserialize_tuple_struct("ActionKey", 2, ActionKeyVisitor)
    }
}
