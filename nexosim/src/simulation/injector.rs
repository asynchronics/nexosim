use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use recycle_box::{RecycleBox, coerce_box};

use crate::model::Model;
use crate::ports::InputFn;
use crate::simulation::queue_items::Event;
use crate::simulation::{Address, EventId};
use crate::util::priority_queue::PriorityQueue;

use super::GLOBAL_ORIGIN_ID;

/// Alias for the scheduler queue type.
///
/// Why use the origin ID as a key? The short answer is that this allows to
/// preserve the relative ordering of events which have the same origin (where
/// the origin is either a model instance or the global scheduler). The
/// preservation of this ordering is implemented by the event loop, which
/// aggregate events with the same origin into single sequential futures, thus
/// ensuring that they are not executed concurrently.
pub(crate) type InjectorQueue = PriorityQueue<usize, InjectorItem>;

pub(crate) enum InjectorItem {
    Event(Event),
    Future(Pin<Box<dyn Future<Output = ()> + Send>>),
}

/// An injector for events to be processed by a model as soon as possible.
///
/// The [`EventInjector::event`] method is similar to
/// [`Context::schedule_event`](crate::model::Context::schedule_event) but is
/// used to request events to be processed as soon as possible rather than at a
/// specific deadline. A `EventInjector` is always associated to a model
/// instance and a specific input method.
pub struct EventInjector<T> {
    queue: Arc<Mutex<InjectorQueue>>,
    origin_id: usize,
    inner: Box<dyn InjectorFutGen<T> + Send + 'static>,
}

impl<T> EventInjector<T>
where
    T: Clone + Send + 'static,
{
    pub(crate) fn new<M, F, S>(
        func: F,
        address: impl Into<Address<M>>,
        queue: Arc<Mutex<InjectorQueue>>,
        origin_id: usize,
    ) -> Self
    where
        M: Model,
        F: for<'a> InputFn<'a, M, T, S> + Clone + Sync,
        S: Send + Sync,
    {
        let sender = address.into().0;
        let inner =
            move |arg: T| async move {
                let _ = sender.send(
                move |model: &mut M,
                      scheduler,
                      env,
                      recycle_box: RecycleBox<()>|
                      -> RecycleBox<dyn Future<Output = ()> + Send + '_> {
                    let fut = func.call(model, arg, scheduler, env);
                    coerce_box!(RecycleBox::recycle(recycle_box, fut))
                }
            ).await;
            };
        let inner = InjectorInner::new(inner);

        Self {
            inner: Box::new(inner),
            queue,
            origin_id,
        }
    }

    pub(crate) fn mapped<M, C, F, U, S>(
        func: F,
        map: C,
        address: impl Into<Address<M>>,
        queue: Arc<Mutex<InjectorQueue>>,
        origin_id: usize,
    ) -> Self
    where
        M: Model,
        C: for<'a> Fn(&'a T) -> U + Clone + Send + Sync + 'static,
        F: for<'a> InputFn<'a, M, U, S> + Clone + Sync,
        U: Send + 'static,
        S: Send + Sync,
    {
        let sender = address.into().0;
        let inner =
            move |arg: T| async move {
                let _ = sender.send(
                move |model: &mut M,
                      scheduler,
                      env,
                      recycle_box: RecycleBox<()>|
                      -> RecycleBox<dyn Future<Output = ()> + Send + '_> {
                    let arg = map(&arg);
                    let fut = func.call(model, arg, scheduler, env);
                    coerce_box!(RecycleBox::recycle(recycle_box, fut))
                }
            ).await;
            };
        let inner = InjectorInner::new(inner);

        Self {
            inner: Box::new(inner),
            queue,
            origin_id,
        }
    }

    /// Injects an event to be processed as soon as possible.
    ///
    /// If a stepping method such as
    /// [`Simulation::step`](crate::simulation::Simulation::step) or
    /// [`Simulation::run`](crate::simulation::Simulation::run) is executed
    /// concurrently, the event will be processed at the deadline set by the
    /// scheduler event or simulation tick that directly follows the one that is
    /// being stepped into.
    ///
    /// If the event is injected while the simulation is at rest, the event will
    /// be processed at the lapse of the next simulation step (next scheduler
    /// event or simulation tick).
    pub fn inject(&self, arg: T) {
        let fut = self.inner.future(arg);
        let mut queue = self.queue.lock().unwrap();
        queue.insert(self.origin_id, InjectorItem::Future(fut));
    }
}

impl<T> fmt::Debug for EventInjector<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventInjector")
            .field("origin_id", &self.origin_id)
            .finish_non_exhaustive()
    }
}

/// An injector for events to be processed as soon as possible.
///
/// An `Injector` is similar to a [`Scheduler`](crate::simulation::Scheduler)
/// but is used to request events to be processed as soon as possible rather
/// than at a specific deadline.
#[derive(Clone)]
pub struct Injector {
    queue: Arc<Mutex<InjectorQueue>>,
}

impl Injector {
    pub(crate) fn new(queue: Arc<Mutex<InjectorQueue>>) -> Self {
        Self { queue }
    }

    /// Injects an event to be processed as soon as possible.
    ///
    /// If a stepping method such as
    /// [`Simulation::step`](crate::simulation::Simulation::step) or
    /// [`Simulation::run`](crate::simulation::Simulation::run) is executed
    /// concurrently, the event will be processed at the deadline set by the
    /// scheduler event or simulation tick that directly follows the one that is
    /// being stepped into.
    ///
    /// If the event is injected while the simulation is at rest, the event will
    /// be processed at the lapse of the next simulation step (next scheduler
    /// event or simulation tick).
    pub fn inject_event<T>(&self, event_id: &EventId<T>, arg: T)
    where
        T: Send + Clone + 'static,
    {
        let event = Event::new(event_id, arg);
        self.inject_built_event(event);
    }

    /// Injects an already built event to be processed as soon as possible.
    pub(crate) fn inject_built_event(&self, event: Event) {
        let mut queue = self.queue.lock().unwrap();
        queue.insert(GLOBAL_ORIGIN_ID, InjectorItem::Event(event));
    }
}

impl fmt::Debug for Injector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Injector").finish_non_exhaustive()
    }
}

/// Private trait that allows type-erased cloneable future generation.
trait InjectorFutGen<T> {
    fn future(&self, arg: T) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
}

/// Typed event future generator.
struct InjectorInner<G, F, T>
where
    G: (FnOnce(T) -> F) + Clone + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    generator: G,
    _marker: PhantomData<(F, T)>,
}
impl<G, F, T> InjectorInner<G, F, T>
where
    G: (FnOnce(T) -> F) + Clone + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    fn new(generator: G) -> Self {
        Self {
            generator,
            _marker: PhantomData,
        }
    }
}
impl<G, F, T> InjectorFutGen<T> for InjectorInner<G, F, T>
where
    G: (FnOnce(T) -> F) + Clone,
    F: Future<Output = ()> + Send + 'static,
    T: Clone + Send + 'static,
{
    fn future(&self, arg: T) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        let f = self.generator.clone()(arg);
        Box::pin(f)
    }
}
