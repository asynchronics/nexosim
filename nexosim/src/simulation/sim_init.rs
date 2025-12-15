use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{de::DeserializeOwned, Serialize};

use crate::channel::ChannelObserver;
use crate::executor::{Executor, SimulationContext};
use crate::model::{Message, Model, ProtoModel, RegisteredModel};
use crate::ports::{
    EventSinkReader, EventSource, InputFn, QuerySource, RegisteredEventSource,
    RegisteredQuerySource,
};
use crate::registry::EndpointRegistry;
use crate::time::{
    AtomicTime, Clock, ClockReader, MonotonicTime, NoClock, SyncStatus, TearableAtomicTime,
};
use crate::util::sync_cell::SyncCell;

use super::{
    add_model, Address, EventId, ExecutionError, GlobalScheduler, InputSource, Mailbox, QueryId,
    SchedulerQueue, SchedulerRegistry, Signal, Simulation, SimulationError,
};

type PostCallback = dyn FnMut(&mut Simulation) -> Result<(), SimulationError> + 'static;

/// Builder for a multi-threaded, discrete-event simulation.
pub struct SimInit {
    executor: Executor,
    scheduler_queue: Arc<Mutex<SchedulerQueue>>,
    scheduler_registry: SchedulerRegistry,
    endpoint_registry: EndpointRegistry,
    time: AtomicTime,
    is_halted: Arc<AtomicBool>,
    is_resumed: Arc<AtomicBool>,
    clock: Box<dyn Clock + 'static>,
    clock_tolerance: Option<Duration>,
    timeout: Duration,
    observers: Vec<(String, Box<dyn ChannelObserver>)>,
    abort_signal: Signal,
    registered_models: Vec<RegisteredModel>,
    post_init_callback: Option<Box<PostCallback>>,
    post_restore_callback: Option<Box<PostCallback>>,
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
        let time = SyncCell::new(TearableAtomicTime::new(MonotonicTime::EPOCH));
        let simulation_context = SimulationContext {
            #[cfg(feature = "tracing")]
            time_reader: time.reader(),
        };

        let abort_signal = Signal::new();
        let executor = if num_threads == 1 {
            Executor::new_single_threaded(simulation_context, abort_signal.clone())
        } else {
            Executor::new_multi_threaded(num_threads, simulation_context, abort_signal.clone())
        };

        Self {
            executor,
            scheduler_queue: Arc::new(Mutex::new(SchedulerQueue::new())),
            scheduler_registry: SchedulerRegistry::default(),
            endpoint_registry: EndpointRegistry::default(),
            time,
            is_halted: Arc::new(AtomicBool::new(false)),
            is_resumed: Arc::new(AtomicBool::new(false)),
            clock: Box::new(NoClock::new()),
            clock_tolerance: None,
            timeout: Duration::ZERO,
            observers: Vec::new(),
            abort_signal,
            registered_models: Vec::new(),
            post_init_callback: None,
            post_restore_callback: None,
        }
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

    /// Registers a callback function executed right after the simulation is
    /// initialized and before it's run.
    ///
    /// Initial event scheduling or input processing is possible at this stage.
    pub fn with_post_init(
        mut self,
        callback: impl FnMut(&mut Simulation) -> Result<(), SimulationError> + 'static,
    ) -> Self {
        self.post_init_callback = Some(Box::new(callback));
        self
    }

    /// Registers a callback function executed right after the simulation is
    /// restored from a persisted state.
    ///
    /// If necessary real-time clock resynchronization can be performed at this
    /// stage. Otherwise simulation actions such as event scheduling or querying
    /// are also possible.
    pub fn with_post_restore(
        mut self,
        callback: impl FnMut(&mut Simulation) -> Result<(), SimulationError> + 'static,
    ) -> Self {
        self.post_restore_callback = Some(Box::new(callback));
        self
    }

    /// Registers `EventSource<T>` as schedulable.
    pub(crate) fn link_event_source<T>(&mut self, source: EventSource<T>) -> EventId<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.scheduler_registry.event_registry.add(source)
    }

    pub(crate) fn link_query_source<T, R>(&mut self, source: QuerySource<T, R>) -> QueryId<T, R>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        self.scheduler_registry.query_registry.add(source)
    }

    /// Returns a clock reader instance, allowing to track simulation
    /// time (once it is initialized).
    pub fn clock_reader(&self) -> ClockReader {
        ClockReader::from_atomic_time_reader(&self.time.reader())
    }

    /// Adds a model and its mailbox to the simulation bench.
    ///
    /// The `name` argument needs not be unique. The use of the dot character in
    /// the name is possible but discouraged as it can cause confusion with the
    /// fully qualified name of a submodel. If an empty string is provided, it
    /// is replaced by the string `<unknown>`.
    pub fn add_model<P>(
        mut self,
        model: P,
        mailbox: Mailbox<P::Model>,
        name: impl Into<String>,
    ) -> Self
    where
        P: ProtoModel,
    {
        let mut name = name.into();
        if name.is_empty() {
            name = String::from("<unknown>");
        };
        self.observers
            .push((name.clone(), Box::new(mailbox.0.observer())));
        let scheduler = GlobalScheduler::new(
            self.scheduler_queue.clone(),
            self.time.reader(),
            self.is_halted.clone(),
        );

        add_model(
            model,
            mailbox,
            name,
            scheduler,
            &mut self.scheduler_registry,
            &self.executor,
            &self.abort_signal,
            &mut self.registered_models,
            self.is_resumed.clone(),
        );

        self
    }

    /// Adds an event source to the endpoint registry.
    ///
    /// If the specified name is already in use for another event source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_event_source<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), ()>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        // TODO refactor `get_event_source` usage?
        let name: String = name.into();
        // Check for duplicates before registering in the scheduler.
        if self.endpoint_registry.get_event_source::<T>(&name).is_ok() {
            return Err(());
        }
        let source = Arc::new(source);
        let event_id = self.scheduler_registry.event_registry.add(source.clone());
        // Should not fail after the check above
        let _ = self.endpoint_registry.add_event_source(
            RegisteredEventSource::from_event_source(source, event_id),
            name,
        );
        Ok(())
    }

    /// Adds an event source to the endpoint registry without requiring a
    /// `Message` implementation for its item type.
    ///
    /// If the specified name is already in use for another event source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_event_source_raw<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), ()>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        // TODO refactor `get_event_source` usage?
        let name: String = name.into();
        // Check for duplicates before registering in the scheduler.
        if self.endpoint_registry.get_event_source::<T>(&name).is_ok() {
            return Err(());
        }
        let source = Arc::new(source);
        let event_id = self.scheduler_registry.event_registry.add(source.clone());
        let _ = self.endpoint_registry.add_event_source_raw(
            RegisteredEventSource::from_event_source(source, event_id),
            name,
        );
        Ok(())
    }

    /// Adds a query source to the endpoint registry.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_query_source<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), ()>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        // TODO see add_event_source remarks
        let name: String = name.into();
        if self
            .endpoint_registry
            .get_query_source::<T, R>(&name)
            .is_ok()
        {
            return Err(());
        }
        let source = Arc::new(source);
        let query_id = self.scheduler_registry.query_registry.add(source.clone());
        let _ = self.endpoint_registry.add_query_source(
            RegisteredQuerySource::from_event_source(source, query_id),
            name,
        );
        Ok(())
    }

    /// Adds a query source to the endpoint registry without requiring `Message`
    /// implementations for its query and response types.
    ///
    /// If the specified name is already in use for another query source, the
    /// source provided as argument is returned in the error.
    pub(crate) fn add_query_source_raw<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), ()>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        // TODO see add_event_source remarks
        let name: String = name.into();
        if self
            .endpoint_registry
            .get_query_source::<T, R>(&name)
            .is_ok()
        {
            return Err(());
        }
        let source = Arc::new(source);
        let query_id = self.scheduler_registry.query_registry.add(source.clone());
        let _ = self.endpoint_registry.add_query_source_raw(
            RegisteredQuerySource::from_event_source(source, query_id),
            name,
        );
        Ok(())
    }

    /// Adds an event sink to the endpoint registry.
    ///
    /// If the specified name is already in use for another event sink, the
    /// event sink provided as argument is returned in the error.
    pub fn add_event_sink<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Message + Serialize,
    {
        self.endpoint_registry.add_event_sink(sink, name)
    }

    /// Adds an event sink to the endpoint registry without requiring a
    /// `Message` implementation for its item type.
    ///
    /// If the specified name is already in use for another event sink, the
    /// event sink provided as argument is returned in the error.
    pub fn add_event_sink_raw<S>(&mut self, sink: S, name: impl Into<String>) -> Result<(), S>
    where
        S: EventSinkReader + Send + Sync + 'static,
        S::Item: Serialize,
    {
        self.endpoint_registry.add_event_sink_raw(sink, name)
    }

    fn build(self) -> (Simulation, EndpointRegistry) {
        let (mut simulation, mut endpoint_registry) = (
            Simulation::new(
                self.executor,
                self.scheduler_queue,
                self.scheduler_registry,
                self.time,
                self.clock,
                self.clock_tolerance,
                self.timeout,
                self.observers,
                self.registered_models,
                self.is_halted,
            ),
            self.endpoint_registry,
        );

        (simulation, endpoint_registry)
    }

    /// Builds a simulation initialized at the specified simulation time,
    /// executing the [`Model::init`] method on all model initializers.
    ///
    /// The simulation object and endpoints registry are returned upon success.
    pub fn init_with_registry(
        mut self,
        start_time: MonotonicTime,
    ) -> Result<(Simulation, EndpointRegistry), SimulationError> {
        self.time.write(start_time);
        if let SyncStatus::OutOfSync(lag) = self.clock.synchronize(start_time) {
            if let Some(tolerance) = &self.clock_tolerance {
                if &lag > tolerance {
                    return Err(ExecutionError::OutOfSync(lag).into());
                }
            }
        }

        let callback = self.post_init_callback.take();
        let (mut simulation, endpoint_registry) = self.build();
        if let Some(mut callback) = callback {
            callback(&mut simulation)?;
        }
        simulation.run()?;

        Ok((simulation, endpoint_registry))
    }

    /// Builds a simulation initialized at the specified simulation time,
    /// executing the [`Model::init`] method on all model initializers.
    ///
    /// The simulation object is returned upon success.
    pub fn init(self, start_time: MonotonicTime) -> Result<Simulation, SimulationError> {
        Ok(self.init_with_registry(start_time)?.0)
    }

    /// Restores a simulation from a previously persisted state, executing the
    /// [`Model::restore`] method on all registered models.
    ///
    /// The simulation object is returned upon success.
    pub fn restore<R: std::io::Read>(
        mut self,
        state: R,
    ) -> Result<(Simulation, EndpointRegistry), SimulationError> {
        self.is_resumed.store(true, Ordering::Relaxed);

        let callback = self.post_restore_callback.take();
        let (mut simulation, endpoint_registry) = self.build();

        simulation.restore(state)?;

        if let Some(mut callback) = callback {
            callback(&mut simulation)?;
        }
        // TODO should run?
        simulation.run()?;

        Ok((simulation, endpoint_registry))
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
