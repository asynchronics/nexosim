//! Discrete-event simulation management.
//!
//! This module contains most notably the [`Simulation`] environment, the
//! [`SimInit`] simulation builder, the [`Mailbox`] and [`Address`] types as
//! well as miscellaneous other types related to simulation management.
//!
//! # Simulation lifecycle
//!
//! The lifecycle of a simulation bench typically comprises the following
//! stages:
//!
//! 1. instantiation of models and their [`Mailbox`]es,
//! 2. connection of the models' output/requestor ports to input/replier ports
//!    using the [`Address`]es of the target models,
//! 3. instantiation of a [`SimInit`] simulation bench builder and migration of
//!    all models and mailboxes to the builder with [`SimInit::add_model`],
//! 4. initialization of a [`Simulation`] instance with [`SimInit::init`],
//!    possibly preceded by the setup of a custom clock with
//!    [`SimInit::with_clock`],
//! 5. discrete-time simulation, which typically involves scheduling events and
//!    incrementing simulation time while observing the models outputs.
//!
//! Most information necessary to run a simulation is available in the root
//! crate [documentation](crate) and in the [`SimInit`] and [`Simulation`]
//! documentation. The next section complement this information with a set of
//! practical recommendations that can help run and troubleshoot simulations.
//!
//! # Practical considerations
//!
//! ## Mailbox capacity
//!
//! A [`Mailbox`] is a buffer that store incoming events and queries for a
//! single model instance. Mailboxes have a bounded capacity, which defaults to
//! [`Mailbox::DEFAULT_CAPACITY`].
//!
//! The capacity is a trade-off: too large a capacity may lead to excessive
//! memory usage, whereas too small a capacity can hamper performance and
//! increase the likelihood of deadlocks (see next section). Note that, because
//! a mailbox may receive events or queries of various sizes, it is actually the
//! largest message sent that ultimately determines the amount of allocated
//! memory.
//!
//! The default capacity should prove a reasonable trade-off in most cases, but
//! for situations where it is not appropriate, it is possible to instantiate
//! mailboxes with a custom capacity by using [`Mailbox::with_capacity`] instead
//! of [`Mailbox::new`].
//!
//! ## Avoiding deadlocks
//!
//! While the underlying architecture of NeXosim—the actor model—should prevent
//! most race conditions (including obviously data races which are not possible
//! in safe Rust) it is still possible in theory to generate deadlocks. Though
//! rare in practice, these may occur due to one of the below:
//!
//! 1. *query loopback*: if a model sends a query which loops back to itself
//!    (either directly or transitively via other models), that model would in
//!    effect wait for its own response and block,
//! 2. *mailbox saturation loopback*: if an asynchronous model method sends in
//!    the same call many events that end up saturating its own mailbox (either
//!    directly or transitively via other models), then any attempt to send
//!    another event would block forever waiting for its own mailbox to free
//!    some space.
//!
//! The first scenario is usually very easy to avoid and is typically the result
//! of an improper assembly of models. Because requestor ports are only used
//! sparingly in idiomatic simulations, this situation should be relatively
//! exceptional.
//!
//! The second scenario is rare in well-behaving models and if it occurs, it is
//! most typically at the very beginning of a simulation when models
//! simultaneously and mutually send events during the call to [`Model::init`].
//! If such a large amount of events is deemed normal behavior, the issue can be
//! remedied by increasing the capacity of the saturated mailboxes.
//!
//! Deadlocks are reported as [`ExecutionError::Deadlock`] errors, which
//! identify all involved models and the count of unprocessed messages (events
//! or requests) in their mailboxes.
mod injector;
mod mailbox;
mod queue_items;
mod scheduler;
mod sim_init;

pub use injector::ModelInjector;
pub use mailbox::{Address, Mailbox};
pub use queue_items::{AutoEventKey, EventId, EventKey, QueryId};
pub use scheduler::{Scheduler, SchedulingError};
pub use sim_init::{
    BenchError, DuplicateEventSinkError, DuplicateEventSourceError, DuplicateQuerySourceError,
    SimInit,
};

pub(crate) use injector::InjectorQueue;
#[cfg(feature = "server")]
pub(crate) use queue_items::Event;
pub(crate) use queue_items::{
    EVENT_KEY_REG, EventIdErased, EventKeyReg, InputSource, QueryIdErased, QueueItem,
    SchedulerRegistry,
};
pub(crate) use scheduler::GlobalScheduler;

use std::any::{Any, TypeId};
use std::cell::Cell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Poll;
use std::time::Duration;
use std::{panic, task};

use pin_project::pin_project;
use recycle_box::{RecycleBox, coerce_box};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use scheduler::{SchedulerKey, SchedulerQueue};

use crate::channel::{ChannelObserver, SendError};
#[cfg(feature = "server")]
use crate::endpoints::{EventSourceEntryAny, QuerySourceEntryAny, ReplyReaderAny};
use crate::executor::{Executor, ExecutorError, Signal};
use crate::model::{BuildContext, Context, Model, ProtoModel, RegisteredModel};
use crate::path::Path;
use crate::ports::{ReplierFn, query_replier};
use crate::time::{AtomicTime, Clock, Deadline, MonotonicTime, SyncStatus, Ticker};
use crate::util::seq_futures::SeqFuture;
use crate::util::serialization::serialization_config;
use crate::util::slot;

thread_local! { pub(crate) static CURRENT_MODEL_ID: Cell<ModelId> = const { Cell::new(ModelId::none()) }; }

/// Simulation environment.
///
/// A `Simulation` is created by calling
/// [`SimInit::init`](crate::simulation::SimInit::init) on a simulation bench
/// initializer. It contains an asynchronous executor that runs all simulation
/// models added beforehand to [`SimInit`].
///
/// A [`Simulation`] object also manages an event scheduling queue and
/// simulation time. The scheduling queue can be accessed from the simulation
/// itself, but also from models via the optional [`&mut
/// Context`](crate::model::Context) argument of input and replier port methods.
/// Likewise, simulation time can be accessed with the [`Simulation::time`]
/// method, or from models with the
/// [`Context::time`](crate::simulation::Context::time) method.
///
/// Events and queries can be scheduled immediately, *i.e.* for the current
/// simulation time, using [`process_event`](Simulation::process_event) and
/// [`send_query`](Simulation::process_query). Calling these methods will block
/// until all computations triggered by such event or query have completed. In
/// the case of queries, the response is returned.
///
/// Events can also be scheduled at a future simulation time using one of the
/// [`schedule_*`](Scheduler::schedule_event) method. These methods queue an
/// event without blocking.
///
/// Finally, the [`Simulation`] instance manages simulation time. A call to
/// [`step`](Simulation::step) will:
///
/// 1. increment simulation time until that of the next scheduled event in
///    chronological order, then
/// 2. call [`Clock::synchronize`] which, unless the simulation is configured to
///    run as fast as possible, blocks until the desired wall clock time, and
///    finally
/// 3. run all computations scheduled for the new simulation time.
///
/// The [`step_until`](Simulation::step_until) method operates similarly but
/// iterates until the target simulation time has been reached.
pub struct Simulation {
    executor: Executor,
    scheduler_queue: Arc<Mutex<SchedulerQueue>>,
    scheduler_registry: SchedulerRegistry,
    injector_queue: Arc<Mutex<InjectorQueue>>,
    time: AtomicTime,
    clock: Box<dyn Clock>,
    clock_tolerance: Option<Duration>,
    ticker: Option<Box<dyn Ticker>>,
    timeout: Duration,
    observers: Vec<(Path, Box<dyn ChannelObserver>)>,
    registered_models: Vec<RegisteredModel>,
    is_halted: Arc<AtomicBool>,
    is_terminated: bool,
}

impl Simulation {
    /// Creates a new `Simulation` with the specified clock.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        executor: Executor,
        scheduler_queue: Arc<Mutex<SchedulerQueue>>,
        scheduler_registry: SchedulerRegistry,
        injector_queue: Arc<Mutex<InjectorQueue>>,
        time: AtomicTime,
        clock: Box<dyn Clock>,
        clock_tolerance: Option<Duration>,
        ticker: Option<Box<dyn Ticker>>,
        timeout: Duration,
        observers: Vec<(Path, Box<dyn ChannelObserver>)>,
        registered_models: Vec<RegisteredModel>,
        is_halted: Arc<AtomicBool>,
    ) -> Self {
        Self {
            executor,
            scheduler_queue,
            scheduler_registry,
            injector_queue,
            time,
            clock,
            clock_tolerance,
            ticker,
            timeout,
            observers,
            registered_models,
            is_halted,
            is_terminated: false,
        }
    }

    /// Reset the simulation clock and (re)set the ticker.
    ///
    /// This can in particular be used to resume a simulation driven by a
    /// real-time clock after it was halted, using a new clock with an update
    /// time reference.
    ///
    /// See also [`SimInit::with_clock`].
    pub fn with_clock(&mut self, clock: impl Clock, ticker: impl Ticker) {
        self.clock = Box::new(clock);
        self.ticker = Some(Box::new(ticker));
    }

    /// Reset the simulation clock and run the simulation in tickless mode.
    ///
    /// This can in particular be used to resume a simulation driven by a
    /// real-time clock after it was halted, using instead a clock running as
    /// fast as possible.
    ///
    /// See also [`SimInit::with_tickless_clock`].
    pub fn with_tickless_clock(&mut self, clock: impl Clock) {
        self.clock = Box::new(clock);
        self.ticker = None;
    }

    /// Sets a timeout for each simulation step.
    ///
    /// The timeout corresponds to the maximum wall clock time allocated for the
    /// completion of a single simulation step before an
    /// [`ExecutionError::Timeout`] error is raised.
    ///
    /// A null duration disables the timeout, which is the default behavior.
    ///
    /// See also [`SimInit::with_timeout`].
    #[cfg(not(target_family = "wasm"))]
    pub fn with_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Returns the current simulation time.
    pub fn time(&self) -> MonotonicTime {
        self.time.read()
    }

    /// Advances simulation time to that of the next scheduled event, processing
    /// that event as well as all other events scheduled for the same time.
    ///
    /// Processing is gated by a (possibly blocking) call to
    /// [`Clock::synchronize`] on the configured simulation clock. This method
    /// blocks until all newly processed events have completed.
    pub fn step(&mut self) -> Result<(), ExecutionError> {
        self.step_to_next(None).map(|_| ())
    }

    /// Iteratively advances the simulation time until the specified deadline,
    /// as if by calling [`Simulation::step`] repeatedly.
    ///
    /// This method blocks until all events scheduled up to the specified target
    /// time have completed. The simulation time upon completion is equal to the
    /// specified target time, whether or not an event was scheduled for that
    /// time.
    pub fn step_until(&mut self, deadline: impl Deadline) -> Result<(), ExecutionError> {
        let now = self.time.read();
        let target_time = deadline.into_time(now);
        if target_time < now {
            return Err(ExecutionError::InvalidDeadline(target_time));
        }
        self.step_until_unchecked(Some(target_time))
    }

    /// Iteratively advances the simulation time, as if by calling
    /// [`Simulation::step`] repeatedly.
    ///
    /// This method blocks until the simulation is halted or all scheduled
    /// events have completed.
    pub fn run(&mut self) -> Result<(), ExecutionError> {
        self.step_until_unchecked(None)
    }

    /// Processes a future immediately, blocking until completion.
    fn process_future(
        &mut self,
        fut: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ExecutionError> {
        self.take_halt_flag()?;
        self.executor.spawn_and_forget(fut);
        self.run_executor()
    }

    /// Processes an event immediately, blocking until completion.
    ///
    /// Simulation time remains unchanged.
    pub fn process_event<T>(&mut self, event_id: &EventId<T>, arg: T) -> Result<(), ExecutionError>
    where
        T: Serialize + DeserializeOwned + Send + Clone + 'static,
    {
        let source = self
            .scheduler_registry
            .get_event_source(&(*event_id).into())
            .ok_or(ExecutionError::InvalidEventId(event_id.0))?;

        let fut = source.future_owned(Box::new(arg), None)?;

        self.process_future(fut)
    }

    /// Processes an event immediately, blocking until completion.
    ///
    /// Simulation time remains unchanged.
    #[cfg(feature = "server")]
    pub(crate) fn process_event_erased(
        &mut self,
        event_source: &dyn EventSourceEntryAny,
        arg: Box<dyn Any>,
    ) -> Result<(), ExecutionError> {
        let source = self
            .scheduler_registry
            .get_event_source(&event_source.get_event_id())
            .ok_or(ExecutionError::InvalidEventId(
                event_source.get_event_id().0,
            ))?;

        let fut = source.future_owned(arg, None)?;

        self.process_future(fut)
    }

    /// Processes a query immediately, blocking until completion.
    ///
    /// Simulation time remains unchanged. If the mailbox targeted by the query
    /// was not found in the simulation, an [`ExecutionError::BadQuery`] is
    /// returned.
    pub fn process_query<T, R>(
        &mut self,
        query_id: &QueryId<T, R>,
        arg: T,
    ) -> Result<R, ExecutionError>
    where
        T: Send + Clone + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = query_replier();

        let source = self
            .scheduler_registry
            .get_query_source(&(*query_id).into())
            .ok_or(ExecutionError::InvalidQueryId(query_id.0))?;

        let fut = source.future(Box::new(arg), Some(Box::new(tx)))?;
        self.process_future(fut)?;

        // If the future resolves successfully it should be
        // guaranteed that the reply is present.
        Ok(rx.read().unwrap().next().unwrap())
    }

    /// Processes a query immediately, blocking until completion.
    ///
    /// Simulation time remains unchanged. If the mailbox targeted by the query
    /// was not found in the simulation, an [`ExecutionError::BadQuery`] is
    /// returned.
    #[cfg(feature = "server")]
    pub(crate) fn process_query_erased(
        &mut self,
        query_source: &dyn QuerySourceEntryAny,
        arg: Box<dyn Any>,
    ) -> Result<Box<dyn ReplyReaderAny>, ExecutionError> {
        let source = self
            .scheduler_registry
            .get_query_source(&query_source.get_query_id())
            .ok_or(ExecutionError::InvalidQueryId(
                query_source.get_query_id().0,
            ))?;

        let (tx, rx) = query_source.replier();

        let fut = source.future(arg, Some(tx))?;
        self.process_future(fut)?;
        Ok(rx)
    }

    // TODO used for serialization / deserialization only - find a way to remove it
    pub(crate) fn process_replier_fn<M, F, T, R, S>(
        &mut self,
        func: F,
        arg: T,
        address: impl Into<Address<M>>,
    ) -> Result<R, ExecutionError>
    where
        M: Model,
        F: for<'a> ReplierFn<'a, M, T, R, S>,
        T: Send + Clone + 'static,
        R: Send + 'static,
    {
        let (reply_writer, mut reply_reader) = slot::slot();
        let sender = address.into().0;

        let fut = async move {
            // Ignore send errors.
            let _ = sender
                .send(
                    move |model: &mut M,
                          scheduler,
                          env,
                          recycle_box: RecycleBox<()>|
                          -> RecycleBox<dyn Future<Output = ()> + Send + '_> {
                        let fut = async move {
                            let reply = func.call(model, arg, scheduler, env).await;
                            let _ = reply_writer.write(reply);
                        };

                        coerce_box!(RecycleBox::recycle(recycle_box, fut))
                    },
                )
                .await;
        };

        self.process_future(fut)?;

        reply_reader
            .try_read()
            .map_err(|_| ExecutionError::BadQuery)
    }

    /// Runs the executor.
    fn run_executor(&mut self) -> Result<(), ExecutionError> {
        if self.is_terminated {
            return Err(ExecutionError::Terminated);
        }

        self.executor.run(self.timeout).map_err(|e| {
            self.is_terminated = true;

            match e {
                ExecutorError::UnprocessedMessages(msg_count) => {
                    let mut deadlock_info = Vec::new();
                    for (model, observer) in &self.observers {
                        let mailbox_size = observer.len();
                        if mailbox_size != 0 {
                            deadlock_info.push(DeadlockInfo {
                                model: model.clone(),
                                mailbox_size,
                            });
                        }
                    }

                    if deadlock_info.is_empty() {
                        ExecutionError::MessageLoss(msg_count)
                    } else {
                        ExecutionError::Deadlock(deadlock_info)
                    }
                }
                ExecutorError::Timeout => ExecutionError::Timeout,
                ExecutorError::Panic(model_id, payload) => {
                    let model = model_id
                        .get()
                        .map(|id| self.registered_models.get(id).unwrap().path.clone());

                    // Filter out panics originating from a `SendError`.
                    if (*payload).type_id() == TypeId::of::<SendError>() {
                        return ExecutionError::NoRecipient { model };
                    }

                    if let Some(model) = model {
                        return ExecutionError::Panic { model, payload };
                    }

                    // The panic is due to an internal issue.
                    panic::resume_unwind(payload);
                }
            }
        })
    }

    /// Blocks until the provided deadline.
    ///
    /// An `ExecutionError::OutOfSync` error is returned if the clock lags by
    /// more than the tolerance.
    fn synchronize(&mut self, deadline: MonotonicTime) -> Result<(), ExecutionError> {
        if let SyncStatus::OutOfSync(lag) = self.clock.synchronize(deadline)
            && let Some(tolerance) = &self.clock_tolerance
            && &lag > tolerance
        {
            self.is_terminated = true;

            return Err(ExecutionError::OutOfSync(lag));
        }

        Ok(())
    }

    /// Advances simulation time to that of the next tick or the next scheduled
    /// event (whichever is earlier) that does not exceed the specified bound,
    /// processing all events scheduled or injected until that time.
    ///
    /// If at least one event or tick satisfies the time bound, the
    /// corresponding new simulation time is returned.
    fn step_to_next(
        &mut self,
        upper_time_bound: Option<MonotonicTime>,
    ) -> Result<Option<MonotonicTime>, ExecutionError> {
        self.take_halt_flag()?;

        if self.is_terminated {
            return Err(ExecutionError::Terminated);
        }

        let upper_time_bound = upper_time_bound.unwrap_or(MonotonicTime::MAX);

        // Closure returning the next key which time stamp is no older than the
        // upper bound, if any. Cancelled events are pulled and discarded.
        let peek_next_key = |scheduler_queue: &mut MutexGuard<SchedulerQueue>| {
            loop {
                match scheduler_queue.peek() {
                    Some((&key, item)) if key.0 <= upper_time_bound => {
                        // Discard and evict cancelled events.
                        if let QueueItem::Event(event) = item
                            && event.is_cancelled()
                        {
                            scheduler_queue.pull();
                        } else {
                            break Some(key);
                        }
                    }
                    _ => break None,
                }
            }
        };

        // Set to simulation time to the next scheduled event or next tick,
        // whichever is earlier.
        let next_tick = self.ticker.as_mut().and_then(|ticker| {
            let tick = ticker.next_tick(self.time.read());
            (tick <= upper_time_bound).then_some(tick)
        });
        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();
        let mut next_key = peek_next_key(&mut scheduler_queue);
        let time = match (next_key, next_tick) {
            (Some(key), Some(tick)) => tick.min(key.0),
            (Some(key), None) => key.0,
            (None, Some(tick)) => tick,
            (None, None) => return Ok(None),
        };

        self.time.write(time);

        let mut has_events = false;

        // Spawn scheduled events matching the current time stamp.
        while next_key.map(|key| key.0 == time).unwrap_or(false) {
            // Merge all events with the same origin in a single future to
            // preserve event ordering.
            let mut event_seq = SeqFuture::new();
            next_key = loop {
                let ((time, origin_id), item) = scheduler_queue.pull().unwrap();

                let fut = match item {
                    QueueItem::Event(event) => {
                        let source = self
                            .scheduler_registry
                            .get_event_source(&event.event_id)
                            .ok_or(ExecutionError::InvalidEventId(event.event_id.0))?;

                        if let Some(period) = event.period {
                            let fut = source.future_borrowed(&*event.arg, event.key.as_ref())?;
                            scheduler_queue
                                .insert((time + period, origin_id), QueueItem::Event(event));
                            fut
                        } else {
                            source.future_owned(event.arg, event.key)?
                        }
                    }
                    QueueItem::Query(query) => {
                        let source = self
                            .scheduler_registry
                            .get_query_source(&query.query_id)
                            .ok_or(ExecutionError::InvalidQueryId(query.query_id.0))?;
                        source.future(query.arg, query.replier)?
                    }
                };

                event_seq.push(fut);

                let key = peek_next_key(&mut scheduler_queue);
                if key != next_key {
                    break key;
                }
            };

            // Spawn a compound future that sequentially polls all events
            // targeting the same mailbox.
            self.executor.spawn_and_forget(event_seq);
            has_events = true;
        }

        // Make sure the scheduler's mutex is released before the potentially
        // blocking call to `synchronize`.
        drop(scheduler_queue);

        // Spawn injector events. The events are assumed to be non-periodic and
        // non-cancellable.
        {
            let mut injector_queue = self.injector_queue.lock().unwrap();

            if let Some(mut origin_id) = injector_queue.peek().map(|item| *item.0) {
                has_events = true;
                let mut event_seq = SeqFuture::new();
                while let Some((id, event)) = injector_queue.pull() {
                    let source = self
                        .scheduler_registry
                        .get_event_source(&event.event_id)
                        .ok_or(ExecutionError::InvalidEventId(event.event_id.0))?;

                    let fut = source.future_owned(event.arg, event.key)?;
                    if id != origin_id {
                        self.executor.spawn_and_forget(event_seq);
                        event_seq = SeqFuture::new();
                        origin_id = id
                    }
                    event_seq.push(fut);
                }

                self.executor.spawn_and_forget(event_seq);
            }
        }

        // Block until the deadline.
        self.synchronize(time)?;

        // Run the executor is necessary.
        if has_events {
            self.run_executor()?;
        }

        Ok(Some(time))
    }

    /// Iteratively advances simulation time and processes all events scheduled
    /// up to the specified target time.
    ///
    /// Once the method returns it is guaranteed that (i) all events scheduled
    /// up to the specified target time have completed and (ii) the final
    /// simulation time matches the target time.
    ///
    /// This method does not check whether the specified time lies in the future
    /// of the current simulation time.
    fn step_until_unchecked(
        &mut self,
        target_time: Option<MonotonicTime>,
    ) -> Result<(), ExecutionError> {
        loop {
            match self.step_to_next(target_time) {
                // The target time was reached exactly.
                Ok(time) if time == target_time => return Ok(()),
                // No events are scheduled before or at the target time.
                Ok(None) => {
                    if let Some(target_time) = target_time {
                        // Update the simulation time.
                        self.time.write(target_time);
                        self.synchronize(target_time)?;
                    }
                    return Ok(());
                }
                Err(e) => return Err(e),
                // The target time was not reached yet.
                _ => {}
            }
        }
    }

    /// Checks whether the halt flag is set and clears it if necessary.
    ///
    /// An `ExecutionError::Halted` error is returned if the flag was set.
    fn take_halt_flag(&mut self) -> Result<(), ExecutionError> {
        if self.is_halted.load(Ordering::Relaxed) {
            self.is_halted.store(false, Ordering::Relaxed);

            return Err(ExecutionError::Halted);
        }
        Ok(())
    }

    /// Returns a scheduler handle.
    pub fn scheduler(&self) -> Scheduler {
        Scheduler::new(
            self.scheduler_queue.clone(),
            self.time.reader(),
            self.is_halted.clone(),
        )
    }

    /// Requests and stores serialized state from each of the models.
    fn save_models(&mut self) -> Result<Vec<Vec<u8>>, ExecutionError> {
        // Temporarily move out of the simulation object.
        let models = self.registered_models.drain(..).collect::<Vec<_>>();
        let mut values = Vec::new();
        for model in models.iter() {
            values.push((model.serialize)(self)?);
        }
        self.registered_models = models;
        Ok(values)
    }

    /// Restore models' state.
    fn restore_models(
        &mut self,
        model_state: Vec<Vec<u8>>,
        event_key_reg: &EventKeyReg,
    ) -> Result<(), ExecutionError> {
        // Temporarily move out of the simulation object.
        let models = self.registered_models.drain(..).collect::<Vec<_>>();
        for (model, state) in models.iter().zip(model_state) {
            (model.deserialize)(self, (state, event_key_reg.clone()))?;
        }
        self.registered_models = models;
        Ok(())
    }

    /// Saves the scheduler queue, maintaining its event order.
    fn save_queue(&self) -> Result<Vec<u8>, ExecutionError> {
        let scheduler_queue = self.scheduler_queue.lock().unwrap();
        let queue = scheduler_queue
            .iter()
            .map(|(k, v)| match v.serialize(&self.scheduler_registry) {
                Ok(v) => Ok((*k, v)),
                Err(e) => Err(e),
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;

        bincode::serde::encode_to_vec(&queue, serialization_config())
            .map_err(|e| SaveError::SchedulerQueueSerializationError { cause: Box::new(e) }.into())
    }

    /// Restores the scheduler queue from the serialized state.
    fn restore_queue(&mut self, state: &[u8]) -> Result<(), ExecutionError> {
        let deserialized: Vec<(SchedulerKey, Vec<u8>)> =
            bincode::serde::decode_from_slice(state, serialization_config())
                .map_err(|e| RestoreError::SchedulerQueueDeserializationError {
                    cause: Box::new(e),
                })?
                .0;

        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();
        scheduler_queue.clear();

        for entry in deserialized {
            scheduler_queue.insert(
                entry.0,
                QueueItem::deserialize(&entry.1, &self.scheduler_registry)?,
            );
        }
        Ok(())
    }

    /// Persists a serialized simulation state.
    /// Saved byte count is returned upon success.
    pub fn save<W: std::io::Write>(&mut self, writer: &mut W) -> Result<usize, ExecutionError> {
        // TODO should call halt first?
        let state = SimulationState {
            models: self.save_models()?,
            scheduler_queue: self.save_queue()?,
            time: self.time(),
        };
        bincode::serde::encode_into_std_write(state, writer, serialization_config())
            .map_err(|e| SaveError::SimulationStateSerializationError { cause: Box::new(e) }.into())
    }

    /// Restore simulation state from a serialized data.
    pub(crate) fn restore<R: std::io::Read>(&mut self, mut state: R) -> Result<(), ExecutionError> {
        let event_key_reg = Arc::new(Mutex::new(HashMap::new()));
        EVENT_KEY_REG.set(&event_key_reg, || {
            let state: SimulationState =
                bincode::serde::decode_from_std_read(&mut state, serialization_config()).map_err(
                    |e| RestoreError::SimulationStateDeserializationError { cause: Box::new(e) },
                )?;

            self.time.write(state.time);
            self.restore_models(state.models, &event_key_reg)?;
            self.restore_queue(&state.scheduler_queue)?;
            Ok::<_, ExecutionError>(())
        })?;
        Ok(())
    }
}

impl fmt::Debug for Simulation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Simulation")
            .field("time", &self.time.read())
            .finish_non_exhaustive()
    }
}

/// Internal helper struct organizing parts of a persisted simulation state.
#[derive(Serialize, Deserialize)]
struct SimulationState {
    models: Vec<Vec<u8>>,
    scheduler_queue: Vec<u8>,
    time: MonotonicTime,
}

/// Information regarding a deadlocked model.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeadlockInfo {
    /// The path to a deadlocked model.
    pub model: Path,
    /// Number of messages in the mailbox.
    pub mailbox_size: usize,
}

/// An error returned upon failure during simulation state store procedure.
#[non_exhaustive]
#[derive(Debug)]
pub enum SaveError {
    /// Serialization of the simulation's config has failed.
    ConfigSerializationError {
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to binary encode model's state.
    ModelSerializationError {
        /// Path to the model.
        model: Path,
        /// Type name of the model.
        type_name: &'static str,
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to serialize an event.
    EventSerializationError {
        /// Event's sourceId.
        event_id: usize,
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to serialize an event.
    QuerySerializationError {
        /// Query's sourceId.
        query_id: usize,
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to serialize the scheduler queue.
    SchedulerQueueSerializationError {
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to serialize the complete simulation state.
    SimulationStateSerializationError {
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to save an event with an unknown id.
    EventNotFound {
        /// Event's sourceId.
        event_id: usize,
    },
    /// Failed attempt to save a query with an unknown id.
    QueryNotFound {
        /// Query's sourceId.
        query_id: usize,
    },
    /// Argument data downcasting to a concrete type has failed.
    ArgumentTypeMismatch {
        /// Expected type name.
        type_name: &'static str,
    },
    /// Failed attempt to serialize an event argument.
    ArgumentSerializationError {
        /// Expected type name.
        type_name: &'static str,
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
}
impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ConfigSerializationError { .. } => f.write_str("config serialization has failed"),
            Self::ModelSerializationError {
                model: path,
                type_name,
                ..
            } => write!(f, "cannot serialize model '{path}': {type_name}"),
            Self::EventSerializationError { event_id, .. } => {
                write!(f, "cannot serialize event {event_id}")
            }
            Self::QuerySerializationError { query_id, .. } => {
                write!(f, "cannot serialize query {query_id}")
            }
            Self::SchedulerQueueSerializationError { .. } => {
                f.write_str("cannot serialize scheduler queue")
            }
            Self::SimulationStateSerializationError { .. } => {
                f.write_str("cannot serialize simulation state")
            }
            Self::EventNotFound { event_id } => {
                write!(f, "serialized event (id {event_id}) cannot be found")
            }
            Self::QueryNotFound { query_id } => {
                write!(f, "serialized query (id {query_id}) cannot be found")
            }
            Self::ArgumentTypeMismatch { type_name } => write!(
                f,
                "type mismatch while casting event argument, expected: {type_name}"
            ),
            Self::ArgumentSerializationError { type_name, .. } => {
                write!(f, "cannot serialize event arg, expected type: {type_name}")
            }
        }
    }
}
impl Error for SaveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ConfigSerializationError { cause } => Some(cause.as_ref()),
            Self::ModelSerializationError { cause, .. } => Some(cause.as_ref()),
            Self::EventSerializationError { cause, .. } => Some(cause.as_ref()),
            Self::QuerySerializationError { cause, .. } => Some(cause.as_ref()),
            Self::SchedulerQueueSerializationError { cause } => Some(cause.as_ref()),
            Self::SimulationStateSerializationError { cause } => Some(cause.as_ref()),
            Self::EventNotFound { .. } => None,
            Self::QueryNotFound { .. } => None,
            Self::ArgumentTypeMismatch { .. } => None,
            Self::ArgumentSerializationError { cause, .. } => Some(cause.as_ref()),
        }
    }
}

/// An error returned upon failure during simulation restore from a saved state.
#[non_exhaustive]
#[derive(Debug)]
pub enum RestoreError {
    /// No simulation configuration was found in the restored state data.
    ConfigMissing,
    /// Failed attempt to deserialize model's state.
    ModelDeserializationError {
        /// Path to the model.
        model: Path,
        /// Type name of the model.
        type_name: &'static str,
        /// Underlying deserialization error
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to serialize model's state.
    ModelSerializationError {
        /// Path to the model.
        model: Path,
        /// Type name of the model.
        type_name: &'static str,
        /// Underlying serialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to deserialize a queue item.
    QueueItemDeserializationError {
        /// Underlying deserialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to deserialize the scheduler queue.
    SchedulerQueueDeserializationError {
        /// Underlying deserialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to deserialize the complete simulation state.
    SimulationStateDeserializationError {
        /// Underlying deserialization error.
        cause: Box<dyn Error + Send>,
    },
    /// Failed attempt to restore an event with an unknown id.
    EventNotFound {
        /// Event's sourceId
        event_id: usize,
    },
    /// Failed attempt to restore a query with an unknown id.
    QueryNotFound {
        /// Query's sourceId
        query_id: usize,
    },
    /// Failed attempt to deserialize an event argument.
    ArgumentDeserializationError {
        /// Expected type name
        type_name: &'static str,
        /// Underlying deserialization error.
        cause: Box<dyn Error + Send>,
    },
}
impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ConfigMissing => f.write_str("simulation config is missing"),
            Self::ModelDeserializationError {
                model, type_name, ..
            } => write!(f, "cannot deserialize model {model}: {type_name}"),
            Self::ModelSerializationError {
                model, type_name, ..
            } => write!(f, "cannot serialize model {model}: {type_name}"),
            Self::QueueItemDeserializationError { .. } => {
                f.write_str("cannot deserialize queue item")
            }
            Self::SchedulerQueueDeserializationError { .. } => {
                f.write_str("cannot deserialize scheduler queue")
            }
            Self::SimulationStateDeserializationError { .. } => {
                f.write_str("cannot deserialize simulation state")
            }
            Self::EventNotFound { event_id } => {
                write!(f, "deserialized event (id {event_id}) cannot be found")
            }
            Self::QueryNotFound { query_id } => {
                write!(f, "deserialized query (id {query_id}) cannot be found")
            }
            Self::ArgumentDeserializationError { type_name, .. } => {
                write!(
                    f,
                    "cannot deserialize event arg, expected type: {type_name}"
                )
            }
        }
    }
}
impl Error for RestoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ConfigMissing => None,
            Self::ModelDeserializationError { cause, .. } => Some(cause.as_ref()),
            Self::ModelSerializationError { cause, .. } => Some(cause.as_ref()),
            Self::QueueItemDeserializationError { cause, .. } => Some(cause.as_ref()),
            Self::SchedulerQueueDeserializationError { cause } => Some(cause.as_ref()),
            Self::SimulationStateDeserializationError { cause } => Some(cause.as_ref()),
            Self::EventNotFound { .. } => None,
            Self::QueryNotFound { .. } => None,
            Self::ArgumentDeserializationError { cause, .. } => Some(cause.as_ref()),
        }
    }
}

/// An error returned upon simulation execution failure.
#[non_exhaustive]
#[derive(Debug)]
pub enum ExecutionError {
    /// The simulation has been intentionally interrupted with a call to
    /// [`Scheduler::halt`].
    ///
    /// The simulation remains in a well-defined state and can be resumed.
    Halted,
    /// The simulation has been terminated due to an earlier deadlock, message
    /// loss, missing recipient, model panic, timeout or synchronization loss.
    Terminated,
    /// The simulation has deadlocked due to the enlisted models.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    Deadlock(Vec<DeadlockInfo>),
    /// One or more message were left unprocessed because the recipient's
    /// mailbox was not migrated to the simulation.
    ///
    /// The payload indicates the number of lost messages.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    MessageLoss(usize),
    /// The recipient of a message does not exists.
    ///
    /// This indicates that the mailbox of the recipient of a message was not
    /// migrated to the simulation and was no longer alive when the message was
    /// sent.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    NoRecipient {
        /// Path to the model that attempted to send a message, or `None` if
        /// the message was sent from the scheduler.
        model: Option<Path>,
    },
    /// A panic was caught during execution.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    Panic {
        /// Path to the panicking model.
        model: Path,
        /// Payload associated with the panic.
        ///
        /// The payload can be usually downcast to a `String` or `&str`. This is
        /// always the case if the panic was triggered by the `panic!` macro,
        /// but panics can in principle emit arbitrary payloads with e.g.
        /// [`panic_any`](std::panic::panic_any).
        payload: Box<dyn Any + Send + 'static>,
    },
    /// The simulation step has failed to complete within the allocated time.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    ///
    /// See also [`SimInit::with_timeout`].
    Timeout,
    /// The simulation has lost synchronization with the clock and lags behind
    /// by the duration given in the payload.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    ///
    /// See also [`SimInit::with_clock_tolerance`].
    OutOfSync(Duration),
    /// The query did not obtain a response because the mailbox targeted by the
    /// query was not found in the simulation.
    ///
    /// This is a non-fatal error.
    BadQuery,
    /// The specified simulation deadline is in the past of the current
    /// simulation time.
    ///
    /// This is a non-fatal error.
    InvalidDeadline(MonotonicTime),
    /// A non-existent event source identifier has been used.
    InvalidEventId(usize),
    /// A non-existent query source identifier has been used.
    InvalidQueryId(usize),
    /// The type of the event argument is invalid.
    InvalidEventType {
        /// The actual event type.
        expected_event_type: &'static str,
    },
    /// The type of the query argument or reply invalid.
    InvalidQueryType {
        /// The actual request type.
        expected_request_type: &'static str,
        /// The actual replier type.
        expected_reply_type: &'static str,
    },
    /// Simulation serialization has failed.
    SaveError(SaveError),
    /// Simulation deserialization has failed.
    RestoreError(RestoreError),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Halted => f.write_str("the simulation has been intentionally stopped"),
            Self::Terminated => f.write_str("the simulation has been terminated"),
            Self::Deadlock(list) => {
                f.write_str(
                    "a simulation deadlock has been detected that involves the following models: ",
                )?;
                let mut first_item = true;
                for info in list {
                    if first_item {
                        first_item = false;
                    } else {
                        f.write_str(", ")?;
                    }
                    write!(
                        f,
                        "'{}' ({} item{} in mailbox)",
                        info.model,
                        info.mailbox_size,
                        if info.mailbox_size == 1 { "" } else { "s" }
                    )?;
                }

                Ok(())
            }
            Self::MessageLoss(count) => {
                write!(f, "{count} messages have been lost")
            }
            Self::NoRecipient{model} => {
                match model {
                    Some(model) => write!(f,
                        "an attempt by model '{model}' to send a message failed because the recipient's mailbox is no longer alive"
                    ),
                    None => f.write_str("an attempt by the scheduler to send a message failed because the recipient's mailbox is no longer alive"),
                }
            }
            Self::Panic{model, payload} => {
                let msg: &str = if let Some(s) = payload.downcast_ref::<&str>() {
                    s
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s
                } else {
                    return write!(f, "model '{model}' has panicked");
                };
                write!(f, "model '{model}' has panicked with the message: '{msg}'")
            }
            Self::Timeout => f.write_str("the simulation step has failed to complete within the allocated time"),
            Self::OutOfSync(lag) => {
                write!(
                    f,
                    "the simulation has lost synchronization and lags behind the clock by '{lag:?}'"
                )
            }
            Self::BadQuery => f.write_str("the query did not return any response; was the target mailbox added to the simulation?"),
            Self::InvalidDeadline(time) => {
                write!(
                    f,
                    "the specified deadline ({time}) lies in the past of the current simulation time"
                )
            }
            Self::InvalidEventId(e) => write!(f, "event source with identifier '{e}' was not found"),
            Self::InvalidQueryId(e) => write!(f, "query source with identifier '{e}' was not found"),
            Self::InvalidEventType {
                expected_event_type,
            } => {
                write!(
                    f,
                    "invalid event type, expected: {expected_event_type}"
                )
            }
            Self::InvalidQueryType {
                expected_request_type,
                expected_reply_type,
            } => {
                write!(
                    f,
                    "invalid query request-reply type pair, expected: ('{expected_request_type}', '{expected_reply_type}')"
                )
            }
            Self::SaveError(o) => write!(f, "saving the simulation state has failed: {o}"),
            Self::RestoreError(o) => write!(f, "restoring the simulation state has failed: {o}"),
        }
    }
}

impl Error for ExecutionError {}

impl From<SaveError> for ExecutionError {
    fn from(e: SaveError) -> Self {
        Self::SaveError(e)
    }
}

impl From<RestoreError> for ExecutionError {
    fn from(e: RestoreError) -> Self {
        Self::RestoreError(e)
    }
}

/// An error returned upon bench building, simulation execution or scheduling
/// failure.
#[non_exhaustive]
#[derive(Debug)]
pub enum SimulationError {
    /// Simulation bench building has failed.
    BenchError(BenchError),
    /// The execution of the simulation has failed.
    ExecutionError(ExecutionError),
    /// An attempt to schedule an item has failed.
    SchedulingError(SchedulingError),
}

impl fmt::Display for SimulationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::BenchError(e) => e.fmt(f),
            Self::ExecutionError(e) => e.fmt(f),
            Self::SchedulingError(e) => e.fmt(f),
        }
    }
}

impl Error for SimulationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BenchError(e) => Some(e),
            Self::ExecutionError(e) => Some(e),
            Self::SchedulingError(e) => Some(e),
        }
    }
}

impl From<BenchError> for SimulationError {
    fn from(e: BenchError) -> Self {
        Self::BenchError(e)
    }
}

impl From<ExecutionError> for SimulationError {
    fn from(e: ExecutionError) -> Self {
        Self::ExecutionError(e)
    }
}

impl From<SchedulingError> for SimulationError {
    fn from(e: SchedulingError) -> Self {
        Self::SchedulingError(e)
    }
}

/// Adds a model and its mailbox to the simulation bench.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_model<P>(
    model: P,
    mailbox: Mailbox<P::Model>,
    path: Path,
    scheduler: GlobalScheduler,
    scheduler_registry: &mut SchedulerRegistry,
    injector: &Arc<Mutex<InjectorQueue>>,
    executor: &Executor,
    abort_signal: &Signal,
    registered_models: &mut Vec<RegisteredModel>,
    is_resumed: Arc<AtomicBool>,
) where
    P: ProtoModel,
{
    #[cfg(feature = "tracing")]
    let span = tracing::span!(target: env!("CARGO_PKG_NAME"), tracing::Level::INFO, "model", path = path.to_string());

    let model_id = ModelId::new(registered_models.len());
    let mut build_cx = BuildContext::new(
        &mailbox,
        &path,
        &scheduler,
        scheduler_registry,
        injector,
        model_id.0,
        executor,
        abort_signal,
        registered_models,
        is_resumed.clone(),
    );

    // The model registry must be built before the call to `ProtoModel::build`
    // because `BuildContext::injector` may be called in the build step and it
    // requires the model register.
    let model_registry = Arc::new(P::Model::register_schedulables(&mut build_cx));
    build_cx.set_model_registry(&model_registry);

    // Build the model.
    let (model, mut env) = model.build(&mut build_cx);

    let address = mailbox.address();
    let mut receiver = mailbox.0;
    let abort_signal = abort_signal.clone();

    registered_models.push(RegisteredModel::new(path.clone(), address.clone()));

    let cx = Context::new(path, scheduler, address, model_id.0, model_registry);
    let fut = async move {
        let mut model = if !is_resumed.load(Ordering::Relaxed) {
            model.init(&cx, &mut env).await.0
        } else {
            model
        };
        while !abort_signal.is_set() && receiver.recv(&mut model, &cx, &mut env).await.is_ok() {}
    };

    #[cfg(not(feature = "tracing"))]
    let fut = ModelFuture::new(fut, model_id);
    #[cfg(feature = "tracing")]
    let fut = ModelFuture::new(fut, model_id, span);

    executor.spawn_and_forget(fut);
}

/// A unique index assigned to a model instance.
///
/// This is a thin wrapper over a `usize` which encodes a lack of value as
/// `usize::MAX`.
#[derive(Copy, Clone, Debug)]
pub(crate) struct ModelId(usize);

impl ModelId {
    const fn none() -> Self {
        Self(usize::MAX)
    }
    fn new(id: usize) -> Self {
        assert_ne!(id, usize::MAX);

        Self(id)
    }
    fn get(&self) -> Option<usize> {
        if self.0 != usize::MAX {
            Some(self.0)
        } else {
            None
        }
    }
}

impl Default for ModelId {
    fn default() -> Self {
        Self(usize::MAX)
    }
}

#[pin_project]
struct ModelFuture<F> {
    #[pin]
    fut: F,
    id: ModelId,
    #[cfg(feature = "tracing")]
    span: tracing::Span,
}

impl<F> ModelFuture<F> {
    #[cfg(not(feature = "tracing"))]
    fn new(fut: F, id: ModelId) -> Self {
        Self { fut, id }
    }
    #[cfg(feature = "tracing")]
    fn new(fut: F, id: ModelId, span: tracing::Span) -> Self {
        Self { fut, id, span }
    }
}

impl<F: Future> Future for ModelFuture<F> {
    type Output = F::Output;

    // Required method
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        #[cfg(feature = "tracing")]
        let _enter = this.span.enter();

        // The current model ID is not set/unset through a guard or scoped TLS
        // because it must survive panics to identify the last model that was
        // polled.
        CURRENT_MODEL_ID.set(*this.id);
        let poll = this.fut.poll(cx);

        // The model ID is unset right after polling so we can distinguish
        // between panics generated by models and panics generated by the
        // executor itself, as in the later case `CURRENT_MODEL_ID.get()` will
        // return `None`.
        CURRENT_MODEL_ID.set(ModelId::none());

        poll
    }
}
