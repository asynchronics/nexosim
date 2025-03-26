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
//! 3. instantiation of a [`SimInit`] simulation builder and migration of all
//!    models and mailboxes to the builder with [`SimInit::add_model`],
//! 4. initialization of a [`Simulation`] instance with [`SimInit::init`],
//!    possibly preceded by the setup of a custom clock with
//!    [`SimInit::set_clock`],
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
//!    (either directly or transitively via other models), that model
//!    would in effect wait for its own response and block,
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
mod mailbox;
mod scheduler;
mod sim_init;

pub(crate) use scheduler::{
    GlobalScheduler, KeyedOnceAction, KeyedPeriodicAction, OnceAction, PeriodicAction,
};

pub use mailbox::{Address, Mailbox};
pub use scheduler::{Action, ActionKey, AutoActionKey, Scheduler, SchedulingError};
pub use sim_init::SimInit;

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
use recycle_box::{coerce_box, RecycleBox};
use serde::{Serialize, Serializer};
use tai_time::TaiTime;

use scheduler::{ScheduledEvent, SchedulerQueue, SerializableEvent};

use crate::channel::{ChannelObserver, SendError};
use crate::executor::{Executor, ExecutorError, Signal};
use crate::model::{BuildContext, Context, Model, ProtoModel};
use crate::ports::{InputFn, ReplierFn};
use crate::registry::EventSourceRegistry;
use crate::time::{AtomicTime, Clock, Deadline, MonotonicTime, SyncStatus};
use crate::util::seq_futures::SeqFuture;
use crate::util::slot;

thread_local! { pub(crate) static CURRENT_MODEL_ID: Cell<ModelId> = const { Cell::new(ModelId::none()) }; }

/// Simulation environment.
///
/// A `Simulation` is created by calling
/// [`SimInit::init`](crate::simulation::SimInit::init) on a simulation
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
    registry: EventSourceRegistry,
    time: AtomicTime,
    clock: Box<dyn Clock>,
    clock_tolerance: Option<Duration>,
    timeout: Duration,
    observers: Vec<(String, Box<dyn ChannelObserver>)>,
    model_names: Vec<String>,
    is_halted: Arc<AtomicBool>,
    is_terminated: bool,
}

impl Simulation {
    /// Creates a new `Simulation` with the specified clock.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        executor: Executor,
        scheduler_queue: Arc<Mutex<SchedulerQueue>>,
        registry: EventSourceRegistry,
        time: AtomicTime,
        clock: Box<dyn Clock + 'static>,
        clock_tolerance: Option<Duration>,
        timeout: Duration,
        observers: Vec<(String, Box<dyn ChannelObserver>)>,
        model_names: Vec<String>,
        is_halted: Arc<AtomicBool>,
    ) -> Self {
        Self {
            executor,
            scheduler_queue,
            registry,
            time,
            clock,
            clock_tolerance,
            timeout,
            observers,
            model_names,
            is_halted,
            is_terminated: false,
        }
    }

    /// Sets a timeout for each simulation step.
    ///
    /// The timeout corresponds to the maximum wall clock time allocated for the
    /// completion of a single simulation step before an
    /// [`ExecutionError::Timeout`] error is raised.
    ///
    /// A null duration disables the timeout, which is the default behavior.
    ///
    /// See also [`SimInit::set_timeout`].
    #[cfg(not(target_family = "wasm"))]
    pub fn set_timeout(&mut self, timeout: Duration) {
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
    /// This method blocks until
    /// * the simulation is halted for real-time clock,
    /// * the simulation is halted or all scheduled events have completed for
    ///   non real-time clock.
    pub fn step_unbounded(&mut self) -> Result<(), ExecutionError> {
        self.step_until_unchecked(None)
    }

    /// Processes an action immediately, blocking until completion.
    ///
    /// Simulation time remains unchanged. The periodicity of the action, if
    /// any, is ignored.
    pub fn process(&mut self, action: Action) -> Result<(), ExecutionError> {
        action.spawn_and_forget(&self.executor);
        self.run()
    }

    /// Processes an event immediately, blocking until completion.
    ///
    /// Simulation time remains unchanged.
    pub fn process_event<M, F, T, S>(
        &mut self,
        func: F,
        arg: T,
        address: impl Into<Address<M>>,
    ) -> Result<(), ExecutionError>
    where
        M: Model,
        F: for<'a> InputFn<'a, M, T, S>,
        T: Send + Clone + 'static,
    {
        let sender = address.into().0;
        let fut = async move {
            // Ignore send errors.
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
        };

        self.executor.spawn_and_forget(fut);
        self.run()
    }

    /// Processes a query immediately, blocking until completion.
    ///
    /// Simulation time remains unchanged. If the mailbox targeted by the query
    /// was not found in the simulation, an [`ExecutionError::BadQuery`] is
    /// returned.
    pub fn process_query<M, F, T, R, S>(
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
                          recycle_box: RecycleBox<()>|
                          -> RecycleBox<dyn Future<Output = ()> + Send + '_> {
                        let fut = async move {
                            let reply = func.call(model, arg, scheduler).await;
                            let _ = reply_writer.write(reply);
                        };

                        coerce_box!(RecycleBox::recycle(recycle_box, fut))
                    },
                )
                .await;
        };

        self.executor.spawn_and_forget(fut);
        self.run()?;

        reply_reader
            .try_read()
            .map_err(|_| ExecutionError::BadQuery)
    }

    /// Runs the executor.
    fn run(&mut self) -> Result<(), ExecutionError> {
        if self.is_terminated {
            return Err(ExecutionError::Terminated);
        }

        if self.is_halted.load(Ordering::Relaxed) {
            self.is_terminated = true;
            return Err(ExecutionError::Halted);
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
                        .map(|id| self.model_names.get(id).unwrap().clone());

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

    /// Advances simulation time to that of the next scheduled action if its
    /// scheduling time does not exceed the specified bound, processing that
    /// action as well as all other actions scheduled for the same time.
    ///
    /// If at least one action was found that satisfied the time bound, the
    /// corresponding new simulation time is returned.
    fn step_to_next(
        &mut self,
        upper_time_bound: Option<MonotonicTime>,
    ) -> Result<Option<MonotonicTime>, ExecutionError> {
        if self.is_terminated {
            return Err(ExecutionError::Terminated);
        }

        if self.is_halted.load(Ordering::Relaxed) {
            self.is_terminated = true;
            return Err(ExecutionError::Halted);
        }

        // Function pulling the next action. If the action is periodic, it is
        // immediately re-scheduled.
        fn pull_next_scheduled_event(
            scheduler_queue: &mut MutexGuard<SchedulerQueue>,
        ) -> ScheduledEvent {
            let ((time, channel_id), event) = scheduler_queue.pull().unwrap();
            // TODO
            // if let Some((action_clone, period)) = event.next() {
            //     scheduler_queue.insert((time + period, channel_id), action_clone);
            // }

            event
        }

        let upper_time_bound = upper_time_bound.unwrap_or(MonotonicTime::MAX);

        // Closure returning the next key which time stamp is no older than the
        // upper bound, if any. Cancelled actions are pulled and discarded.
        let peek_next_key = |scheduler_queue: &mut MutexGuard<SchedulerQueue>| {
            loop {
                match scheduler_queue.peek() {
                    Some((&key, action)) if key.0 <= upper_time_bound => {
                        if !action.is_cancelled() {
                            break Some(key);
                        }
                        // Discard cancelled actions.
                        scheduler_queue.pull();
                    }
                    _ => break None,
                }
            }
        };

        // Move to the next scheduled time.
        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();
        let mut current_key = match peek_next_key(&mut scheduler_queue) {
            Some(key) => key,
            None => return Ok(None),
        };
        self.time.write(current_key.0);

        loop {
            let event = pull_next_scheduled_event(&mut scheduler_queue);
            let mut next_key = peek_next_key(&mut scheduler_queue);
            if next_key != Some(current_key) {
                // Since there are no other actions with the same origin and the
                // same time, the action is spawned immediately.

                let source = self
                    .registry
                    .get(&event.source)
                    .ok_or(ExecutionError::InvalidEvent(event.source.to_string()))?;
                // action.spawn_and_forget(&self.executor);
                let fut = source.into_future(&*event.arg);
                let mut action_sequence = SeqFuture::new();
                action_sequence.push(fut);
                self.executor.spawn_and_forget(action_sequence);
                // self.executor.spawn_and_forget(Box::pin(&*fut));
            } else {
                // To ensure that their relative order of execution is
                // preserved, all actions with the same origin are executed
                // sequentially within a single compound future.
                // TODO
                // let mut action_sequence = SeqFuture::new();
                // action_sequence.push(action.into_future());
                // loop {
                //     let action = pull_next_action(&mut scheduler_queue);
                //     action_sequence.push(action.into_future());
                //     next_key = peek_next_key(&mut scheduler_queue);
                //     if next_key != Some(current_key) {
                //         break;
                //     }
                // }

                // Spawn a compound future that sequentially polls all actions
                // targeting the same mailbox.
                // self.executor.spawn_and_forget(action_sequence);
            }

            current_key = match next_key {
                // If the next action is scheduled at the same time, update the
                // key and continue.
                Some(k) if k.0 == current_key.0 => k,
                // Otherwise wait until all actions have completed and return.
                _ => {
                    drop(scheduler_queue); // make sure the queue's mutex is released.

                    let current_time = current_key.0;
                    if let SyncStatus::OutOfSync(lag) = self.clock.synchronize(current_time) {
                        if let Some(tolerance) = &self.clock_tolerance {
                            if &lag > tolerance {
                                self.is_terminated = true;

                                return Err(ExecutionError::OutOfSync(lag));
                            }
                        }
                    }
                    self.run()?;

                    return Ok(Some(current_time));
                }
            };
        }
    }

    /// Iteratively advances simulation time and processes all actions scheduled
    /// up to the specified target time.
    ///
    /// Once the method returns it is guaranteed that (i) all actions scheduled
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
                // No actions are scheduled before or at the target time.
                Ok(None) => {
                    if let Some(target_time) = target_time {
                        // Update the simulation time.
                        self.time.write(target_time);
                        if let SyncStatus::OutOfSync(lag) = self.clock.synchronize(target_time) {
                            if let Some(tolerance) = &self.clock_tolerance {
                                if &lag > tolerance {
                                    self.is_terminated = true;

                                    return Err(ExecutionError::OutOfSync(lag));
                                }
                            }
                        }
                    }
                    return Ok(());
                }
                Err(e) => return Err(e),
                // The target time was not reached yet.
                _ => {}
            }
        }
    }

    pub fn serializable_queue(&self) -> impl Serialize {
        let queue = self.scheduler_queue.lock().unwrap();

        let mut v = Vec::new();
        for a in queue.iter() {
            let source = self.registry.get(&a.value.source).unwrap();
            let arg = source.serialize_arg(&*a.value.arg);
            v.push((
                a.key,
                a.epoch,
                SerializableEvent {
                    source: a.value.source.to_string(),
                    arg,
                },
            ));
        }
        v
    }

    pub fn restore_deserialized_queue(
        &self,
        v: Vec<((TaiTime<0>, usize), f64, SerializableEvent)>,
    ) {
        let mut queue = self.scheduler_queue.lock().unwrap();
        // TODO add drain or clear
        while let Some(_) = queue.pull() {}

        for entry in v {
            let source = self.registry.get(&entry.2.source).unwrap();
            let arg = source.deserialize_arg(&entry.2.arg);
            queue.insert(
                entry.0,
                ScheduledEvent {
                    source: entry.2.source,
                    arg,
                },
            );
        }
    }

    /// Returns a scheduler handle.
    #[cfg(feature = "server")]
    pub(crate) fn scheduler(&self) -> Scheduler {
        Scheduler::new(
            self.scheduler_queue.clone(),
            self.time.reader(),
            self.is_halted.clone(),
        )
    }
}

impl fmt::Debug for Simulation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Simulation")
            .field("time", &self.time.read())
            .finish_non_exhaustive()
    }
}

/// Information regarding a deadlocked model.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeadlockInfo {
    /// The fully qualified name of a deadlocked model.
    ///
    /// This is the name of the model, if relevant prepended by the
    /// dot-separated names of all parent models.
    pub model: String,
    /// Number of messages in the mailbox.
    pub mailbox_size: usize,
}

/// An error returned upon simulation execution failure.
#[non_exhaustive]
#[derive(Debug)]
pub enum ExecutionError {
    /// The simulation has been intentionally stopped.
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
        /// The fully qualified name of the model that attempted to send a
        /// message, or `None` if the message was sent from the scheduler.
        ///
        /// The fully qualified name is made of the unqualified model name, if
        /// relevant prepended by the dot-separated names of all parent models.
        model: Option<String>,
    },
    /// A panic was caught during execution.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    Panic {
        /// The fully qualified name of the panicking model.
        ///
        /// The fully qualified name is made of the unqualified model name, if
        /// relevant prepended by the dot-separated names of all parent models.
        model: String,
        /// The payload associated with the panic.
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
    /// See also [`SimInit::set_timeout`] and [`Simulation::set_timeout`].
    Timeout,
    /// The simulation has lost synchronization with the clock and lags behind
    /// by the duration given in the payload.
    ///
    /// This is a fatal error: any subsequent attempt to run the simulation will
    /// return an [`ExecutionError::Terminated`] error.
    ///
    /// See also [`SimInit::set_clock_tolerance`].
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
    // TODO
    InvalidEvent(String),
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
                write!(f, "{} messages have been lost", count)
            }
            Self::NoRecipient{model} => {
                match model {
                    Some(model) => write!(f,
                        "an attempt by model '{}' to send a message failed because the recipient's mailbox is no longer alive",
                        model
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
                    return write!(f, "model '{}' has panicked", model);
                };
                write!(f, "model '{}' has panicked with the message: '{}'", model, msg)
            }
            Self::Timeout => f.write_str("the simulation step has failed to complete within the allocated time"),
            Self::OutOfSync(lag) => {
                write!(
                    f,
                    "the simulation has lost synchronization and lags behind the clock by '{:?}'",
                    lag
                )
            }
            Self::BadQuery => f.write_str("the query did not return any response; was the target mailbox added to the simulation?"),
            Self::InvalidDeadline(time) => {
                write!(
                    f,
                    "the specified deadline ({}) lies in the past of the current simulation time",
                    time
                )
            }
            Self::InvalidEvent(e) => write!(
                f,
                "Invalid event: {}",
                e
            )
        }
    }
}

impl Error for ExecutionError {}

/// An error returned upon simulation execution or scheduling failure.
#[derive(Debug)]
pub enum SimulationError {
    /// The execution of the simulation failed.
    ExecutionError(ExecutionError),
    /// An attempt to schedule an item failed.
    SchedulingError(SchedulingError),
}

impl fmt::Display for SimulationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ExecutionError(e) => e.fmt(f),
            Self::SchedulingError(e) => e.fmt(f),
        }
    }
}

impl Error for SimulationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ExecutionError(e) => e.source(),
            Self::SchedulingError(e) => e.source(),
        }
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
pub(crate) fn add_model<P: ProtoModel>(
    model: P,
    mailbox: Mailbox<P::Model>,
    name: String,
    scheduler: GlobalScheduler,
    executor: &Executor,
    abort_signal: &Signal,
    model_names: &mut Vec<String>,
) {
    #[cfg(feature = "tracing")]
    let span = tracing::span!(target: env!("CARGO_PKG_NAME"), tracing::Level::INFO, "model", name);

    let mut build_cx = BuildContext::new(
        &mailbox,
        &name,
        &scheduler,
        executor,
        abort_signal,
        model_names,
    );
    let model = model.build(&mut build_cx);

    let address = mailbox.address();
    let mut receiver = mailbox.0;
    let abort_signal = abort_signal.clone();
    let mut cx = Context::new(name.clone(), scheduler, address);
    let fut = async move {
        let mut model = model.init(&mut cx).await.0;
        while !abort_signal.is_set() && receiver.recv(&mut model, &mut cx).await.is_ok() {}
    };

    let model_id = ModelId::new(model_names.len());
    model_names.push(name);

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
