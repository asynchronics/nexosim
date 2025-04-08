use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::channel::ChannelObserver;
use crate::executor::Executor;
use crate::model::{Model, ProtoModel};
use crate::ports::EventSource;
use crate::time::{AtomicTime, Clock, MonotonicTime, NoClock, SyncStatus, TearableAtomicTime};
use crate::util::sync_cell::SyncCell;

use super::{
    add_model, ActionKeyReg, ExecutionError, GlobalScheduler, Mailbox, RegisteredModel, Scheduler,
    SchedulerQueue, Signal, Simulation, SimulationContext, SourceId,
};

/// Builder for a multi-threaded, discrete-event simulation.
pub struct SimInit {
    executor: Executor,
    simulation_context: SimulationContext,
    scheduler_queue: Arc<Mutex<SchedulerQueue>>,
    time: AtomicTime,
    is_running: Arc<AtomicBool>,
    is_resumed: Arc<AtomicBool>,
    clock: Box<dyn Clock + 'static>,
    clock_tolerance: Option<Duration>,
    timeout: Duration,
    observers: Vec<(String, Box<dyn ChannelObserver>)>,
    abort_signal: Signal,
    registered_models: Vec<RegisteredModel>,
}

impl SimInit {
    /// Creates a builder for a multithreaded simulation running on all
    /// available logical threads.
    pub fn new() -> Self {
        Self::with_num_threads(num_cpus::get())
    }

    /// Creates a builder for a simulation running on the specified number of
    /// threads.
    ///
    /// Note that the number of worker threads is automatically constrained to
    /// be between 1 and `usize::BITS` (inclusive). It is always set to 1 on
    /// `wasm` targets.
    pub fn with_num_threads(num_threads: usize) -> Self {
        let num_threads = if cfg!(target_family = "wasm") {
            1
        } else {
            num_threads.clamp(1, usize::BITS as usize)
        };

        let scheduler_queue = Arc::new(Mutex::new(SchedulerQueue::new()));
        let is_running = Arc::new(AtomicBool::new(false));
        let time = SyncCell::new(TearableAtomicTime::new(MonotonicTime::EPOCH));

        let action_key_reg = Arc::new(Mutex::new(HashMap::new()));
        let scheduler =
            GlobalScheduler::new(scheduler_queue.clone(), time.reader(), is_running.clone());

        let simulation_context = SimulationContext {
            #[cfg(feature = "tracing")]
            time_reader: time.reader(),
            scheduler,
            action_key_reg,
        };

        let abort_signal = Signal::new();
        let executor = if num_threads == 1 {
            Executor::new_single_threaded(simulation_context.clone(), abort_signal.clone())
        } else {
            Executor::new_multi_threaded(
                num_threads,
                simulation_context.clone(),
                abort_signal.clone(),
            )
        };

        Self {
            executor,
            scheduler_queue,
            simulation_context,
            time,
            is_running,
            is_resumed: Arc::new(AtomicBool::new(false)),
            clock: Box::new(NoClock::new()),
            clock_tolerance: None,
            timeout: Duration::ZERO,
            observers: Vec::new(),
            abort_signal,
            registered_models: Vec::new(),
        }
    }

    /// Adds a model and its mailbox to the simulation bench.
    ///
    /// The `name` argument needs not be unique. The use of the dot character in
    /// the name is possible but discouraged as it can cause confusion with the
    /// fully qualified name of a submodel. If an empty string is provided, it
    /// is replaced by the string `<unknown>`.
    pub fn add_model<P: ProtoModel>(
        mut self,
        model: P,
        environment: <P::Model as Model>::Environment,
        mailbox: Mailbox<P::Model>,
        name: impl Into<String>,
    ) -> Self
    where
        <P as ProtoModel>::Model: Serialize + DeserializeOwned,
    {
        let mut name = name.into();
        if name.is_empty() {
            name = String::from("<unknown>");
        };
        self.observers
            .push((name.clone(), Box::new(mailbox.0.observer())));

        add_model(
            model,
            environment,
            mailbox,
            name,
            self.simulation_context.scheduler.clone(),
            &self.executor,
            &self.abort_signal,
            &mut self.registered_models,
            self.is_resumed.clone(),
        );

        self
    }

    /// Synchronizes the simulation with the provided [`Clock`].
    ///
    /// If the clock isn't explicitly set then the default [`NoClock`] is used,
    /// resulting in the simulation running as fast as possible.
    pub fn set_clock(mut self, clock: impl Clock + 'static) -> Self {
        self.clock = Box::new(clock);

        self
    }

    /// Specifies a tolerance for clock synchronization.
    ///
    /// When a clock synchronization tolerance is set, then any report of
    /// synchronization loss by [`Clock::synchronize`] that exceeds the
    /// specified tolerance will trigger an [`ExecutionError::OutOfSync`] error.
    pub fn set_clock_tolerance(mut self, tolerance: Duration) -> Self {
        self.clock_tolerance = Some(tolerance);

        self
    }

    /// Sets a timeout for the call to [`SimInit::init`] and for any subsequent
    /// simulation step.
    ///
    /// The timeout corresponds to the maximum wall clock time allocated for the
    /// completion of a single simulation step before an
    /// [`ExecutionError::Timeout`] error is raised.
    ///
    /// A null duration disables the timeout, which is the default behavior.
    ///
    /// See also [`Simulation::set_timeout`].
    #[cfg(not(target_family = "wasm"))]
    pub fn set_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;

        self
    }

    pub fn register_event_source<T>(&self, source: EventSource<T>) -> SourceId<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        let mut queue = self.scheduler_queue.lock().unwrap();
        queue.registry.add(source)
    }

    fn build(self) -> (Simulation, Scheduler) {
        let scheduler = Scheduler::new(
            self.simulation_context.scheduler.scheduler_queue.clone(),
            self.time.reader(),
            self.is_running.clone(),
        );
        let simulation = Simulation::new(
            self.executor,
            self.simulation_context.clone(),
            self.scheduler_queue,
            self.time,
            self.clock,
            self.clock_tolerance,
            self.timeout,
            self.observers,
            self.registered_models,
            self.is_running,
        );
        (simulation, scheduler)
    }

    /// Builds a simulation initialized at the specified simulation time,
    /// executing the [`Model::init`](crate::model::Model::init) method on all
    /// model initializers.
    ///
    /// The simulation object and its associated scheduler are returned upon
    /// success.
    pub fn init(
        mut self,
        start_time: MonotonicTime,
    ) -> Result<(Simulation, Scheduler), ExecutionError> {
        self.time.write(start_time);
        if let SyncStatus::OutOfSync(lag) = self.clock.synchronize(start_time) {
            if let Some(tolerance) = &self.clock_tolerance {
                if &lag > tolerance {
                    return Err(ExecutionError::OutOfSync(lag));
                }
            }
        }
        let (mut simulation, scheduler) = self.build();
        simulation.run()?;

        Ok((simulation, scheduler))
    }

    pub fn restore(mut self, state: Vec<u8>) -> Result<(Simulation, Scheduler), ExecutionError> {
        self.is_resumed.store(true, Ordering::Relaxed);
        let (mut simulation, scheduler) = self.build();
        simulation.restore_state(state);
        // TODO verify if run should be called?
        simulation.run()?;

        Ok((simulation, scheduler))
    }
}

impl Default for SimInit {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SimInit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimInit").finish_non_exhaustive()
    }
}
