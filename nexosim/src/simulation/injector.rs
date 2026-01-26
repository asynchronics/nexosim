use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::model::{Model, ModelRegistry, SchedulableId};
use crate::simulation::queue_items::Event;
use crate::util::priority_queue::PriorityQueue;

/// Alias for the scheduler queue type.
///
/// Why use the origin ID as a key? The short answer is that this allows to
/// preserve the relative ordering of events which have the same origin (where
/// the origin is either a model instance or the global scheduler). The
/// preservation of this ordering is implemented by the event loop, which
/// aggregate events with the same origin into single sequential futures, thus
/// ensuring that they are not executed concurrently.
pub(crate) type InjectorQueue = PriorityQueue<usize, Event>;

/// An injector for events to be processed by a model as soon as possible.
///
/// A `ModelInjector` is similar to a
/// [`Scheduler`](crate::simulation::Scheduler) but is used to request events to
/// be processed as soon as possible rather than at a specific deadline. A
/// `ModelInjector` is always associated to a model instance.
#[derive(Clone)]
pub struct ModelInjector<M: Model> {
    queue: Arc<Mutex<InjectorQueue>>,
    origin_id: usize,
    model_registry: Arc<ModelRegistry>,
    _model: PhantomData<M>,
}

impl<M: Model> ModelInjector<M> {
    pub(crate) fn new(
        queue: Arc<Mutex<InjectorQueue>>,
        origin_id: usize,
        model_registry: Arc<ModelRegistry>,
    ) -> Self {
        Self {
            queue,
            origin_id,
            model_registry,
            _model: PhantomData,
        }
    }

    /// Injects an event to be processed as soon as possible.
    ///
    /// The event will be processed at the next simulation step, whether the
    /// step coincides with a scheduled event or a simulation tick.
    pub fn inject_event<T>(&self, schedulable_id: &SchedulableId<M, T>, arg: T)
    where
        T: Send + Clone + 'static,
    {
        let mut queue = self.queue.lock().unwrap();
        let event = Event::new(&schedulable_id.source_id(&self.model_registry), arg);
        queue.insert(self.origin_id, event);
    }
}

impl<M: Model> fmt::Debug for ModelInjector<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelInjector")
            .field("origin_id", &self.origin_id)
            .finish_non_exhaustive()
    }
}
