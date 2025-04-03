use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use bincode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ports::EventSource;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct SourceId(pub usize);

#[derive(Default)]
// TODO use a vec since the key is usize?
pub(crate) struct SchedulerSourceRegistry(HashMap<SourceId, Box<dyn SchedulerEventSource>>);
impl SchedulerSourceRegistry {
    pub(crate) fn add<T>(&mut self, source: EventSource<T>) -> SourceId
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        let source_id = SourceId(self.0.len());
        self.0.insert(source_id, Box::new(source));
        source_id
    }
    pub(crate) fn get(&self, source_id: &SourceId) -> Option<&dyn SchedulerEventSource> {
        self.0.get(source_id).map(|s| s.as_ref())
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
        // TODO check if the standard config suits us best
        // TODO unwrap
        let arg: T = bincode::serde::decode_from_slice(serialized_arg, bincode::config::standard())
            .unwrap()
            .0;
        Box::new(arg)
    }
    fn serialize_arg(&self, arg: &dyn Any) -> Vec<u8> {
        // TODO unwrap
        let value = arg.downcast_ref::<T>().unwrap();
        bincode::serde::encode_to_vec(value, bincode::config::standard()).unwrap()
    }
    fn into_future(&self, arg: &dyn Any) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(EventSource::into_future(
            self,
            // TODO error handling
            arg.downcast_ref::<T>().unwrap().clone(),
        ))
    }
}

#[derive(Debug)]
pub(crate) struct ScheduledEvent {
    pub source_id: SourceId,
    pub arg: Box<dyn Any + Send>,
    pub period: Option<Duration>,
}
impl ScheduledEvent {
    pub(crate) fn once(source_id: SourceId, arg: Box<dyn Any + Send>) -> Self {
        Self {
            source_id,
            arg,
            period: None,
        }
    }
    pub(crate) fn periodic(
        source_id: SourceId,
        arg: Box<dyn Any + Send>,
        period: Duration,
    ) -> Self {
        Self {
            source_id,
            arg,
            period: Some(period),
        }
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        false
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SerializableEvent {
    source_id: SourceId,
    arg: Vec<u8>,
    period: Option<Duration>,
}
impl SerializableEvent {
    pub fn from_scheduled_event(
        event: &ScheduledEvent,
        registry: &SchedulerSourceRegistry,
    ) -> Self {
        let source = registry.get(&event.source_id).unwrap();
        let arg = source.serialize_arg(&*event.arg);
        Self {
            source_id: event.source_id,
            arg,
            period: event.period,
        }
    }
}
