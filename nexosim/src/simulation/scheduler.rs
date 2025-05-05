//! Scheduling functions and types.
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use pin_project::pin_project;
use recycle_box::{coerce_box, RecycleBox};
use serde::{de::DeserializeOwned, Serialize};

use crate::channel::Sender;
use crate::executor::Executor;
use crate::model::Model;
use crate::ports::{EventSource, InputFn};
use crate::simulation::events::{EventKey, ScheduledEvent, SchedulerSourceRegistry, SourceId};
use crate::time::{AtomicTimeReader, Deadline, MonotonicTime};
use crate::util::priority_queue::PriorityQueue;

use crate::util::serialization::serialization_config;
#[cfg(all(test, not(nexosim_loom)))]
use crate::{time::TearableAtomicTime, util::sync_cell::SyncCell};

use super::ExecutionError;

// Usize::MAX - 1 is used as plain usize::MAX is used e.g. to mark a missing
// ModelId.
const GLOBAL_SCHEDULER_ORIGIN_ID: usize = usize::MAX - 1;

/// A global simulation scheduler.
///
/// A `Scheduler` can be `Clone`d and sent to other threads.
#[derive(Clone)]
pub struct Scheduler(GlobalScheduler);

impl Scheduler {
    pub(crate) fn new(
        scheduler_queue: Arc<Mutex<SchedulerQueue>>,
        time: AtomicTimeReader,
        is_halted: Arc<AtomicBool>,
    ) -> Self {
        Self(GlobalScheduler::new(scheduler_queue, time, is_halted))
    }

    /// Returns the current simulation time.
    ///
    /// # Examples
    ///
    /// ```
    /// use nexosim::simulation::Scheduler;
    /// use nexosim::time::MonotonicTime;
    ///
    /// fn is_third_millennium(scheduler: &Scheduler) -> bool {
    ///     let time = scheduler.time();
    ///     time >= MonotonicTime::new(978307200, 0).unwrap()
    ///         && time < MonotonicTime::new(32535216000, 0).unwrap()
    /// }
    /// ```
    pub fn time(&self) -> MonotonicTime {
        self.0.time()
    }

    /// Schedules an event at a future time.
    ///
    /// An error is returned if the specified time is not in the future of the
    /// current simulation time.
    ///
    /// Events scheduled for the same time and targeting the same model are
    /// guaranteed to be processed according to the scheduling order.
    pub fn schedule_event<T>(
        &self,
        deadline: impl Deadline,
        source_id: SourceId<T>,
        arg: T,
    ) -> Result<(), SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        self.0
            .schedule_event_from(deadline, source_id, arg, GLOBAL_SCHEDULER_ORIGIN_ID)
    }

    /// Schedules a cancellable event at a future time and returns an event key.
    ///
    /// An error is returned if the specified time is not in the future of the
    /// current simulation time.
    ///
    /// Events scheduled for the same time and targeting the same model are
    /// guaranteed to be processed according to the scheduling order.
    pub fn schedule_keyed_event<T>(
        &self,
        deadline: impl Deadline,
        source_id: SourceId<T>,
        arg: T,
    ) -> Result<EventKey, SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        self.0
            .schedule_keyed_event_from(deadline, source_id, arg, GLOBAL_SCHEDULER_ORIGIN_ID)
    }

    /// Schedules a periodically recurring event at a future time.
    ///
    /// An error is returned if the specified time is not in the future of the
    /// current simulation time or if the specified period is null.
    ///
    /// Events scheduled for the same time and targeting the same model are
    /// guaranteed to be processed according to the scheduling order.
    pub fn schedule_periodic_event<T>(
        &self,
        deadline: impl Deadline,
        period: Duration,
        source_id: SourceId<T>,
        arg: T,
    ) -> Result<(), SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        self.0.schedule_periodic_event_from(
            deadline,
            period,
            source_id,
            arg,
            GLOBAL_SCHEDULER_ORIGIN_ID,
        )
    }

    /// Schedules a cancellable, periodically recurring event at a future time
    /// and returns an event key.
    ///
    /// An error is returned if the specified time is not in the future of the
    /// current simulation time or if the specified period is null.
    ///
    /// Events scheduled for the same time and targeting the same model are
    /// guaranteed to be processed according to the scheduling order.
    pub fn schedule_keyed_periodic_event<T>(
        &self,
        deadline: impl Deadline,
        period: Duration,
        source_id: SourceId<T>,
        arg: T,
    ) -> Result<EventKey, SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        self.0.schedule_keyed_periodic_event_from(
            deadline,
            period,
            source_id,
            arg,
            GLOBAL_SCHEDULER_ORIGIN_ID,
        )
    }

    /// Requests the simulation to be interrupted at the earliest opportunity.
    ///
    /// If a multi-step method such as
    /// [`Simulation::step_until`](crate::simulation::Simulation::step_until) or
    /// [`Simulation::step_unbounded`](crate::simulation::Simulation::step_unbounded)
    /// is concurrently being executed, this will cause such method to return
    /// before it steps to next scheduler deadline (if any) with
    /// [`ExecutionError::Halted`](crate::simulation::ExecutionError::Halted).
    ///
    /// Otherwise, this will cause the next call to a `Simulation::step*` or
    /// `Simulation::process*` method to return immediately with
    /// [`ExecutionError::Halted`](crate::simulation::ExecutionError::Halted).
    ///
    /// In all cases, once
    /// [`ExecutionError::Halted`](crate::simulation::ExecutionError::Halted) is
    /// returned, the `halt` flag is cleared and the simulation can be resumed
    /// at any moment with another call to one of the `Simulation::step*` or
    /// `Simulation::process*` methods.
    pub fn halt(&mut self) {
        self.0.halt()
    }
}

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scheduler")
            .field("time", &self.time())
            .finish_non_exhaustive()
    }
}

/// Error returned when the scheduled time or the repetition period are invalid.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SchedulingError {
    /// The scheduled time does not lie in the future of the current simulation
    /// time.
    InvalidScheduledTime,
    /// The repetition period is zero.
    NullRepetitionPeriod,
}

impl fmt::Display for SchedulingError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidScheduledTime => write!(
                fmt,
                "the scheduled time should be in the future of the current simulation time"
            ),
            Self::NullRepetitionPeriod => write!(fmt, "the repetition period cannot be zero"),
        }
    }
}

impl Error for SchedulingError {}

/// A possibly periodic, possibly cancellable action that can be scheduled or
/// processed immediately.
///
/// `Actions` can be created from an [`EventSource`](crate::ports::EventSource)
/// or [`QuerySource`](crate::ports::QuerySource). They can be used to schedule
/// events and requests with [`Scheduler::schedule`], or to process events and
/// requests immediately with
/// [`Simulation::process`](crate::simulation::Simulation::process).
pub struct Action {
    inner: Box<dyn ActionInner>,
}

impl Action {
    /// Creates a new `Action` from an `ActionInner`.
    pub(crate) fn new<S: ActionInner>(s: S) -> Self {
        Self { inner: Box::new(s) }
    }

    /// Reports whether the action was cancelled.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// If this is a periodic action, returns a boxed clone of this action and
    /// its repetition period; otherwise returns `None`.
    pub(crate) fn next(&self) -> Option<(Action, Duration)> {
        self.inner
            .next()
            .map(|(inner, period)| (Self { inner }, period))
    }

    /// Returns a boxed future that performs the action.
    pub(crate) fn into_future(self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        self.inner.into_future()
    }

    /// Spawns the future that performs the action onto the provided executor.
    ///
    /// This method is typically more efficient that spawning the boxed future
    /// from `into_future` since it can directly spawn the unboxed future.
    pub(crate) fn spawn_and_forget(self, executor: &Executor) {
        self.inner.spawn_and_forget(executor)
    }
}

impl fmt::Debug for Action {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SchedulableEvent").finish_non_exhaustive()
    }
}

/// Alias for the scheduler queue type.
///
/// Why use both time and origin ID as the key? The short answer is that this
/// allows to preserve the relative ordering of events which have the same
/// origin (where the origin is either a model instance or the global
/// scheduler). The preservation of this ordering is implemented by the event
/// loop, which aggregate events with the same origin into single sequential
/// futures, thus ensuring that they are not executed concurrently.
pub(crate) struct SchedulerQueue {
    inner: PriorityQueue<SchedulerKey, ScheduledEvent>,
    pub(crate) registry: SchedulerSourceRegistry,
}
impl SchedulerQueue {
    pub(crate) fn new() -> Self {
        Self {
            inner: PriorityQueue::new(),
            registry: SchedulerSourceRegistry::default(),
        }
    }

    pub(crate) fn insert(&mut self, key: SchedulerKey, event: ScheduledEvent) {
        self.inner.insert(key, event);
    }

    pub(crate) fn peek(&self) -> Option<(&SchedulerKey, &ScheduledEvent)> {
        self.inner.peek()
    }

    pub(crate) fn pull(&mut self) -> Option<(SchedulerKey, ScheduledEvent)> {
        self.inner.pull()
    }

    pub(crate) fn serialize(&self) -> Result<Vec<u8>, ExecutionError> {
        let queue = self
            .inner
            .iter()
            .map(|(k, v)| match v.serialize(&self.registry) {
                Ok(v) => Ok((*k, v)),
                Err(e) => Err(e),
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;

        bincode::serde::encode_to_vec(&queue, serialization_config())
            .map_err(|_| ExecutionError::SerializationError)
    }

    pub(crate) fn restore(&mut self, state: &[u8]) -> Result<(), ExecutionError> {
        let deserialized: Vec<(SchedulerKey, Vec<u8>)> =
            bincode::serde::decode_from_slice(state, serialization_config())
                .map_err(|_| ExecutionError::SerializationError)?
                .0;

        self.inner.clear();

        for entry in deserialized {
            self.inner.insert(
                entry.0,
                ScheduledEvent::deserialize(&entry.1, &self.registry)?,
            );
        }

        Ok(())
    }
}

pub(crate) type SchedulerKey = (MonotonicTime, usize);

/// Internal implementation of the global scheduler.
#[derive(Clone)]
pub(crate) struct GlobalScheduler {
    scheduler_queue: Arc<Mutex<SchedulerQueue>>,
    time: AtomicTimeReader,
    is_halted: Arc<AtomicBool>,
}

impl GlobalScheduler {
    pub(crate) fn new(
        scheduler_queue: Arc<Mutex<SchedulerQueue>>,
        time: AtomicTimeReader,
        is_halted: Arc<AtomicBool>,
    ) -> Self {
        Self {
            scheduler_queue,
            time,
            is_halted,
        }
    }

    /// Returns the current simulation time.
    pub(crate) fn time(&self) -> MonotonicTime {
        // We use `read` rather than `try_read` because the scheduler can be
        // sent to another thread than the simulator's and could thus
        // potentially see a torn read if the simulator increments time
        // concurrently. The chances of this happening are very small since
        // simulation time is not changed frequently.
        self.time.read()
    }

    // TODO docs
    pub(crate) fn register_source<T>(&self, source: EventSource<T>) -> SourceId<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        let mut queue = self.scheduler_queue.lock().unwrap();
        queue.registry.add(source)
    }

    /// Schedules an event identified by its origin at a future time.
    pub(crate) fn schedule_event_from<T>(
        &self,
        deadline: impl Deadline,
        source_id: SourceId<T>,
        arg: T,
        origin_id: usize,
    ) -> Result<(), SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        // The scheduler queue must always be locked when reading the time (see
        // `schedule_from`).
        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();
        let now = self.time();
        let time = deadline.into_time(now);
        if now >= time {
            return Err(SchedulingError::InvalidScheduledTime);
        }

        let event = ScheduledEvent::new(source_id, arg);
        scheduler_queue.insert((time, origin_id), event);

        Ok(())
    }

    /// Schedules a cancellable event identified by its origin at a future time
    /// and returns an event key.
    pub(crate) fn schedule_keyed_event_from<T>(
        &self,
        deadline: impl Deadline,
        source_id: SourceId<T>,
        arg: T,
        origin_id: usize,
    ) -> Result<EventKey, SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        let event_key = EventKey::new();

        // The scheduler queue must always be locked when reading the time (see
        // `schedule_from`).
        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();
        let now = self.time();
        let time = deadline.into_time(now);
        if now >= time {
            return Err(SchedulingError::InvalidScheduledTime);
        }

        let event = ScheduledEvent::new(source_id, arg).with_key(event_key.clone());
        scheduler_queue.insert((time, origin_id), event);

        Ok(event_key)
    }

    /// Schedules a periodically recurring event identified by its origin at a
    /// future time.
    pub(crate) fn schedule_periodic_event_from<T>(
        &self,
        deadline: impl Deadline,
        period: Duration,
        source_id: SourceId<T>,
        arg: T,
        origin_id: usize,
    ) -> Result<(), SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        if period.is_zero() {
            return Err(SchedulingError::NullRepetitionPeriod);
        }

        // The scheduler queue must always be locked when reading the time (see
        // `schedule_from`).
        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();
        let now = self.time();
        let time = deadline.into_time(now);
        if now >= time {
            return Err(SchedulingError::InvalidScheduledTime);
        }

        let event = ScheduledEvent::new(source_id, arg).with_period(period);
        scheduler_queue.insert((time, origin_id), event);

        Ok(())
    }

    /// Schedules a cancellable, periodically recurring event identified by its
    /// origin at a future time and returns an event key.
    pub(crate) fn schedule_keyed_periodic_event_from<T>(
        &self,
        deadline: impl Deadline,
        period: Duration,
        source_id: SourceId<T>,
        arg: T,
        origin_id: usize,
    ) -> Result<EventKey, SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        if period.is_zero() {
            return Err(SchedulingError::NullRepetitionPeriod);
        }
        let event_key = EventKey::new();

        // The scheduler queue must always be locked when reading the time (see
        // `schedule_from`).
        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();
        let now = self.time();
        let time = deadline.into_time(now);
        if now >= time {
            return Err(SchedulingError::InvalidScheduledTime);
        }

        let event = ScheduledEvent::new(source_id, arg)
            .with_period(period)
            .with_key(event_key.clone());
        scheduler_queue.insert((time, origin_id), event);

        Ok(event_key)
    }

    /// Requests the simulation to return as early as possible upon the
    /// completion of the current time step.
    pub(crate) fn halt(&mut self) {
        self.is_halted.store(true, Ordering::Relaxed);
    }
}

impl fmt::Debug for GlobalScheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchedulerInner")
            .field("time", &self.time())
            .finish_non_exhaustive()
    }
}

/// Trait abstracting over the inner type of an action.
pub(crate) trait ActionInner: Send + 'static {
    /// Reports whether the action was cancelled.
    fn is_cancelled(&self) -> bool;

    /// If this is a periodic action, returns a boxed clone of this action and
    /// its repetition period; otherwise returns `None`.
    fn next(&self) -> Option<(Box<dyn ActionInner>, Duration)>;

    /// Returns a boxed future that performs the action.
    fn into_future(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>>;

    /// Spawns the future that performs the action onto the provided executor.
    ///
    /// This method is typically more efficient that spawning the boxed future
    /// from `into_future` since it can directly spawn the unboxed future.
    fn spawn_and_forget(self: Box<Self>, executor: &Executor);
}

/// An object that can be converted to a future performing a single
/// non-cancellable action.
///
/// Note that this particular action is in fact already a future: since the
/// future cannot be cancelled and the action does not need to be cloned,
/// there is no need to defer the construction of the future. This makes
/// `into_future` a trivial cast, which saves a boxing operation.
#[pin_project]
pub(crate) struct OnceAction<F> {
    #[pin]
    fut: F,
}

impl<F> OnceAction<F>
where
    F: Future<Output = ()> + Send + 'static,
{
    /// Constructs a new `OnceAction`.
    pub(crate) fn new(fut: F) -> Self {
        OnceAction { fut }
    }
}

impl<F> Future for OnceAction<F>
where
    F: Future,
{
    type Output = F::Output;

    #[inline(always)]
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().fut.poll(cx)
    }
}

impl<F> ActionInner for OnceAction<F>
where
    F: Future<Output = ()> + Send + 'static,
{
    fn is_cancelled(&self) -> bool {
        false
    }
    fn next(&self) -> Option<(Box<dyn ActionInner>, Duration)> {
        None
    }
    fn into_future(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        // No need for boxing, type coercion is enough here.
        Box::into_pin(self)
    }
    fn spawn_and_forget(self: Box<Self>, executor: &Executor) {
        executor.spawn_and_forget(*self);
    }
}

/// An object that can be converted to a future performing a non-cancellable,
/// periodic action.
pub(crate) struct PeriodicAction<G, F>
where
    G: (FnOnce() -> F) + Clone + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    /// A cloneable generator for the associated future.
    gen: G,
    /// The action repetition period.
    period: Duration,
}

impl<G, F> PeriodicAction<G, F>
where
    G: (FnOnce() -> F) + Clone + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    /// Constructs a new `PeriodicAction`.
    pub(crate) fn new(gen: G, period: Duration) -> Self {
        Self { gen, period }
    }
}

impl<G, F> ActionInner for PeriodicAction<G, F>
where
    G: (FnOnce() -> F) + Clone + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    fn is_cancelled(&self) -> bool {
        false
    }
    fn next(&self) -> Option<(Box<dyn ActionInner>, Duration)> {
        let event = Box::new(Self::new(self.gen.clone(), self.period));

        Some((event, self.period))
    }
    fn into_future(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin((self.gen)())
    }
    fn spawn_and_forget(self: Box<Self>, executor: &Executor) {
        executor.spawn_and_forget((self.gen)());
    }
}

/// An object that can be converted to a future performing a single, cancellable
/// action.
pub(crate) struct KeyedOnceAction<G, F>
where
    G: (FnOnce(ActionKey) -> F) + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    /// A generator for the associated future.
    gen: G,
    /// The event cancellation key.
    event_key: ActionKey,
}

impl<G, F> KeyedOnceAction<G, F>
where
    G: (FnOnce(ActionKey) -> F) + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    /// Constructs a new `KeyedOnceAction`.
    pub(crate) fn new(gen: G, event_key: ActionKey) -> Self {
        Self { gen, event_key }
    }
}

impl<G, F> ActionInner for KeyedOnceAction<G, F>
where
    G: (FnOnce(ActionKey) -> F) + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    fn is_cancelled(&self) -> bool {
        self.event_key.is_cancelled()
    }
    fn next(&self) -> Option<(Box<dyn ActionInner>, Duration)> {
        None
    }
    fn into_future(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin((self.gen)(self.event_key))
    }
    fn spawn_and_forget(self: Box<Self>, executor: &Executor) {
        executor.spawn_and_forget((self.gen)(self.event_key));
    }
}

/// An object that can be converted to a future performing a periodic,
/// cancellable action.
pub(crate) struct KeyedPeriodicAction<G, F>
where
    G: (FnOnce(ActionKey) -> F) + Clone + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    /// A cloneable generator for associated future.
    gen: G,
    /// The repetition period.
    period: Duration,
    /// The event cancellation key.
    event_key: ActionKey,
}

impl<G, F> KeyedPeriodicAction<G, F>
where
    G: (FnOnce(ActionKey) -> F) + Clone + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    /// Constructs a new `KeyedPeriodicAction`.
    pub(crate) fn new(gen: G, period: Duration, event_key: ActionKey) -> Self {
        Self {
            gen,
            period,
            event_key,
        }
    }
}

impl<G, F> ActionInner for KeyedPeriodicAction<G, F>
where
    G: (FnOnce(ActionKey) -> F) + Clone + Send + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    fn is_cancelled(&self) -> bool {
        self.event_key.is_cancelled()
    }
    fn next(&self) -> Option<(Box<dyn ActionInner>, Duration)> {
        let event = Box::new(Self::new(
            self.gen.clone(),
            self.period,
            self.event_key.clone(),
        ));

        Some((event, self.period))
    }
    fn into_future(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin((self.gen)(self.event_key))
    }
    fn spawn_and_forget(self: Box<Self>, executor: &Executor) {
        executor.spawn_and_forget((self.gen)(self.event_key));
    }
}

/// Asynchronously sends a non-cancellable event to a model input.
pub(crate) async fn process_event<M, F, T, S>(func: F, arg: T, sender: Sender<M>)
where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S>,
    T: Send + 'static,
{
    let _ = sender
        .send(
            move |model: &mut M,
                  scheduler,
                  recycle_box: RecycleBox<()>|
                  -> RecycleBox<dyn Future<Output = ()> + Send + '_> {
                let fut = func.call(model, arg, scheduler);

                coerce_box!(RecycleBox::recycle(recycle_box, fut))
            },
        )
        .await;
}

/// Asynchronously sends a cancellable event to a model input.
pub(crate) async fn send_keyed_event<M, F, T, S>(
    event_key: ActionKey,
    func: F,
    arg: T,
    sender: Sender<M>,
) where
    M: Model,
    F: for<'a> InputFn<'a, M, T, S>,
    T: Send + Clone + 'static,
{
    let _ = sender
        .send(
            move |model: &mut M,
                  scheduler,
                  recycle_box: RecycleBox<()>|
                  -> RecycleBox<dyn Future<Output = ()> + Send + '_> {
                let fut = async move {
                    // Only perform the call if the event wasn't cancelled.
                    if !event_key.is_cancelled() {
                        func.call(model, arg, scheduler).await;
                    }
                };

                coerce_box!(RecycleBox::recycle(recycle_box, fut))
            },
        )
        .await;
}

#[cfg(all(test, not(nexosim_loom)))]
impl GlobalScheduler {
    /// Creates a dummy scheduler for testing purposes.
    pub(crate) fn new_dummy() -> Self {
        let dummy_priority_queue = Arc::new(Mutex::new(SchedulerQueue::new()));
        let dummy_time = SyncCell::new(TearableAtomicTime::new(MonotonicTime::EPOCH)).reader();
        let dummy_running = Arc::new(AtomicBool::new(false));
        GlobalScheduler::new(dummy_priority_queue, dummy_time, dummy_running)
    }
}
