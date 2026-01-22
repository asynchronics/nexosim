use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{fmt, mem};

use serde::{Serialize, de::DeserializeOwned};

use crate::channel::ChannelObserver;
use crate::endpoints::{
    Endpoints, EventSinkInfoRegistry, EventSinkRegistry, EventSourceRegistry, QuerySourceRegistry,
};
use crate::executor::{Executor, SimulationContext};
use crate::model::{Message, ProtoModel, RegisteredModel};
use crate::path::Path;
use crate::ports::{EventSinkReader, EventSource, QuerySource};
use crate::time::{
    AtomicTime, Clock, ClockReader, MonotonicTime, NoClock, SyncStatus, TearableAtomicTime,
};
use crate::util::sync_cell::SyncCell;

use super::{
    EventId, ExecutionError, GlobalScheduler, Mailbox, QueryId, SchedulerQueue, SchedulerRegistry,
    Signal, Simulation, SimulationError, add_model,
};

type PostCallback = dyn FnOnce(&mut Simulation) -> Result<(), SimulationError> + Send + 'static;

/// Builder for a multi-threaded, discrete-event simulation.
pub struct SimInit {
    executor: Executor,
    scheduler_queue: Arc<Mutex<SchedulerQueue>>,
    scheduler_registry: SchedulerRegistry,
    event_sink_registry: EventSinkRegistry,
    event_sink_info_registry: EventSinkInfoRegistry,
    event_source_registry: EventSourceRegistry,
    query_source_registry: QuerySourceRegistry,
    time: AtomicTime,
    is_halted: Arc<AtomicBool>,
    is_resumed: Arc<AtomicBool>,
    clock: Box<dyn Clock + 'static>,
    clock_tolerance: Option<Duration>,
    timeout: Duration,
    observers: Vec<(Path, Box<dyn ChannelObserver>)>,
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
            event_sink_registry: EventSinkRegistry::default(),
            event_sink_info_registry: EventSinkInfoRegistry::default(),
            event_source_registry: EventSourceRegistry::default(),
            query_source_registry: QuerySourceRegistry::default(),
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
    /// initialized and before it starts.
    ///
    /// Initial event scheduling or input processing is possible at this stage.
    pub fn with_post_init(
        mut self,
        callback: impl FnOnce(&mut Simulation) -> Result<(), SimulationError> + Send + 'static,
    ) -> Self {
        self.post_init_callback = Some(Box::new(callback));
        self
    }

    /// Registers a callback function executed right after the simulation is
    /// restored from a persisted state and before it resumes.
    ///
    /// If necessary, real-time clock resynchronization can be performed at this
    /// stage. Otherwise simulation actions such as event scheduling or querying
    /// are also possible.
    pub fn with_post_restore(
        mut self,
        callback: impl FnOnce(&mut Simulation) -> Result<(), SimulationError> + Send + 'static,
    ) -> Self {
        self.post_restore_callback = Some(Box::new(callback));
        self
    }

    /// Converts an event source to an [`EventId`] that can later be used to
    /// schedule and process events within the simulation instance being built.
    pub(crate) fn link_event_source<T>(&mut self, source: EventSource<T>) -> EventId<T>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.scheduler_registry.add_event_source(source)
    }

    /// Converts a query source to a [`QueryId`] that can later be used to
    /// schedule events within the simulation instance being built.
    pub(crate) fn link_query_source<T, R>(&mut self, source: QuerySource<T, R>) -> QueryId<T, R>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Send + 'static,
    {
        self.scheduler_registry.add_query_source(source)
    }

    /// Returns a simulation clock reader.
    ///
    /// Note that the time returned by the clock reader is meaningless before
    /// the simulation starts.
    pub fn clock_reader(&self) -> ClockReader {
        ClockReader::from_atomic_time_reader(&self.time.reader())
    }

    /// Adds a model and its mailbox to the simulation bench.
    ///
    /// The `name` argument defines the [`Path`] to this top-level model and the
    /// root path of its submodels. Because model paths are used for logging and
    /// error reports, the use of unique names is recommended.
    pub fn add_model<P>(mut self, model: P, mailbox: Mailbox<P::Model>, name: &str) -> Self
    where
        P: ProtoModel,
    {
        let path: Path = name.into();
        self.observers
            .push((path.clone(), Box::new(mailbox.0.observer())));
        let scheduler = GlobalScheduler::new(
            self.scheduler_queue.clone(),
            self.time.reader(),
            self.is_halted.clone(),
        );

        add_model(
            model,
            mailbox,
            path,
            scheduler,
            &mut self.scheduler_registry,
            &self.executor,
            &self.abort_signal,
            &mut self.registered_models,
            self.is_resumed.clone(),
        );

        self
    }

    /// Adds an event sink to the endpoint registry under the provided path.
    ///
    /// Note that this method is exclusively meant for the implementation of
    /// custom event sinks. Dedicated sink builder functions such as
    /// [event_queue_endpoint](crate::ports::event_queue_endpoint) should be
    /// preferred in all other cases.
    ///
    /// An error is returned if the path is already used by another event sink.
    /// The error is convertible to an [`BenchError`].
    pub fn bind_event_sink<S, T>(
        &mut self,
        sink: S,
        path: impl Into<Path>,
    ) -> Result<(), DuplicateEventSinkError>
    where
        S: EventSinkReader<T> + Send + Sync + 'static,
        T: Message + Serialize + 'static,
    {
        let path = path.into();

        self.event_sink_info_registry
            .register::<T>(path.clone())
            .map_err(|path| DuplicateEventSinkError { path })?;

        self.event_sink_registry
            .add(sink, path)
            .map_err(|(path, _)| DuplicateEventSinkError { path })
    }

    /// Adds an event sink to the endpoint registry under the provided path,
    /// without requiring a [`Message`] implementation for its item type.
    ///
    /// Note that this method is exclusively meant for the implementation of
    /// custom event sinks. Dedicated sink builder functions such as
    /// [event_queue_endpoint](crate::ports::event_queue_endpoint) should be
    /// preferred in all other cases.
    ///
    /// An error is returned if the path is already used by another event sink.
    /// The error is convertible to an [`BenchError`].
    pub fn bind_event_sink_raw<S, T>(
        &mut self,
        sink: S,
        path: impl Into<Path>,
    ) -> Result<(), DuplicateEventSinkError>
    where
        S: EventSinkReader<T> + Send + Sync + 'static,
        T: Serialize + 'static,
    {
        let path = path.into();

        self.event_sink_info_registry
            .register_raw(path.clone())
            .map_err(|path| DuplicateEventSinkError { path })?;

        self.event_sink_registry
            .add(sink, path)
            .map_err(|(path, _)| DuplicateEventSinkError { path })
    }

    /// Builds a simulation initialized at the specified simulation time,
    /// executing the [`Model::init`](crate::model::Model::init) method on all
    /// model initializers.
    ///
    /// The simulation object is returned upon success.
    pub fn init(mut self, start_time: MonotonicTime) -> Result<Simulation, SimulationError> {
        self.time.write(start_time);
        if let SyncStatus::OutOfSync(lag) = self.clock.synchronize(start_time) {
            if let Some(tolerance) = &self.clock_tolerance {
                if &lag > tolerance {
                    return Err(ExecutionError::OutOfSync(lag).into());
                }
            }
        }

        let callback = self.post_init_callback.take();
        let mut simulation = self.build();
        if let Some(callback) = callback {
            callback(&mut simulation)?;
        }
        simulation.run_executor()?;

        Ok(simulation)
    }

    /// Restores a simulation from a previously persisted state, executing the
    /// [`Model::restore`](crate::model::Model::restore) method on all
    /// registered models.
    ///
    /// The simulation object is returned upon success.
    pub fn restore<R: std::io::Read>(mut self, state: R) -> Result<Simulation, SimulationError> {
        self.is_resumed.store(true, Ordering::Relaxed);

        let callback = self.post_restore_callback.take();

        let mut simulation = self.build();

        simulation.restore(state)?;

        if let Some(callback) = callback {
            callback(&mut simulation)?;
        }
        // TODO should run?
        simulation.run_executor()?;

        Ok(simulation)
    }

    /// Adds an event source to the endpoint registry under the provided path.
    ///
    /// If the path is already used by another event source, the source provided
    /// as argument is returned in the error. The error is convertible to an
    /// [`BenchError`].
    pub(crate) fn bind_event_source<T>(
        &mut self,
        source: EventSource<T>,
        path: Path,
    ) -> Result<(), DuplicateEventSourceError<T>>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry
            .add(source, path, &mut self.scheduler_registry)
            .map_err(|(path, source)| DuplicateEventSourceError { path, source })
    }

    /// Adds an event source to the endpoint registry under the provided path,
    /// without requiring a [`Message`] implementation for its item type.
    ///
    /// If the path is already used by another event source, the source provided
    /// as argument is returned in the error. The error is convertible to an
    /// [`BenchError`].
    pub(crate) fn bind_event_source_raw<T>(
        &mut self,
        source: EventSource<T>,
        path: Path,
    ) -> Result<(), DuplicateEventSourceError<T>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry
            .add_raw(source, path, &mut self.scheduler_registry)
            .map_err(|(path, source)| DuplicateEventSourceError { path, source })
    }

    /// Adds a query source to the endpoint registry under the provided path.
    ///
    /// If the path is already used by another query source, the source provided
    /// as argument is returned in the error. The error is convertible to an
    /// [`BenchError`].
    pub(crate) fn bind_query_source<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        path: Path,
    ) -> Result<(), DuplicateQuerySourceError<QuerySource<T, R>>>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.query_source_registry
            .add(source, path, &mut self.scheduler_registry)
            .map_err(|(path, source)| DuplicateQuerySourceError { path, source })
    }

    /// Adds a query source to the endpoint registry under the provided path,
    /// without requiring [`Message`] implementations for its query and response
    /// types.
    ///
    /// If the path is already used by another query source, the source provided
    /// as argument is returned in the error. The error is convertible to an
    /// [`BenchError`].
    pub(crate) fn bind_query_source_raw<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        path: Path,
    ) -> Result<(), DuplicateQuerySourceError<QuerySource<T, R>>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.query_source_registry
            .add_raw(source, path, &mut self.scheduler_registry)
            .map_err(|(path, source)| DuplicateQuerySourceError { path, source })
    }

    /// Take ownership of the current endpoint directory.
    ///
    /// This leaves the `SimInit` instance with an empty endpoint directory.
    pub fn take_endpoints(&mut self) -> Endpoints {
        Endpoints::new(
            mem::take(&mut self.event_sink_registry),
            mem::take(&mut self.event_sink_info_registry),
            mem::take(&mut self.event_source_registry),
            mem::take(&mut self.query_source_registry),
        )
    }

    /// Builds a [`Simulation`] from this [`SimInit`] instance.
    fn build(self) -> Simulation {
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
        )
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

/// Error returned when the construction of a simulation bench fails.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BenchError {
    /// An attempt to add an input or event source failed because the provided
    /// path is already in use by another event source.
    DuplicateEventSource(Path),
    /// An attempt to add a query source failed because the provided path is
    /// already in use by another query source.
    DuplicateQuerySource(Path),
    /// An attempt to add an event sink failed because the provided path is
    /// already in use by another event sink.
    DuplicateEventSink(Path),
}

impl fmt::Display for BenchError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateEventSource(path) => write!(
                fmt,
                "cannot add input or event source under `{path}`: this path is already in use by another event source",
            ),
            Self::DuplicateQuerySource(path) => write!(
                fmt,
                "cannot add query source under `{path}`: this path is already in use by another query source",
            ),
            Self::DuplicateEventSink(path) => write!(
                fmt,
                "cannot add event sink under `{path}`: this path is already in use by another event sink",
            ),
        }
    }
}
impl Error for BenchError {}

/// Error returned when attempting to bind an event source to an existing path.
pub struct DuplicateEventSourceError<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    /// Path to the event source.
    pub path: Path,
    /// The event source.
    pub source: EventSource<T>,
}
impl<T> fmt::Display for DuplicateEventSourceError<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "cannot add event source under `{}`: this path is already in use by another event source",
            self.path
        )
    }
}
impl<T> fmt::Debug for DuplicateEventSourceError<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateEventSource")
            .field("path", &self.path)
            .field("source", &"<EventSource<T>>")
            .finish()
    }
}
impl<T> Error for DuplicateEventSourceError<T> where
    T: Serialize + DeserializeOwned + Clone + Send + 'static
{
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static> From<DuplicateEventSourceError<T>>
    for BenchError
{
    fn from(e: DuplicateEventSourceError<T>) -> Self {
        Self::DuplicateEventSource(e.path)
    }
}

/// Error returned when attempting to bind a query source to an existing path.
pub struct DuplicateQuerySourceError<S> {
    /// Path to the query source.
    pub path: Path,
    /// The query source.
    pub source: S,
}
impl<S> fmt::Display for DuplicateQuerySourceError<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "cannot add query source under `{}`: this path is already in use by another query sink",
            self.path
        )
    }
}
impl<S> fmt::Debug for DuplicateQuerySourceError<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateQuerySource")
            .field("path", &self.path)
            .field("source", &"<QuerySource<T, R>>")
            .finish()
    }
}
impl<S> Error for DuplicateQuerySourceError<S> {}

impl<S> From<DuplicateQuerySourceError<S>> for BenchError {
    fn from(e: DuplicateQuerySourceError<S>) -> Self {
        Self::DuplicateQuerySource(e.path)
    }
}

/// Error returned when attempting to bind an event sink to an existing path.
#[derive(Debug)]
pub struct DuplicateEventSinkError {
    /// Path to the event sink.
    pub path: Path,
}
impl fmt::Display for DuplicateEventSinkError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "cannot add event sink under `{}`: this path is already in use by another event sink",
            self.path
        )
    }
}
impl Error for DuplicateEventSinkError {}

impl From<DuplicateEventSinkError> for BenchError {
    fn from(e: DuplicateEventSinkError) -> Self {
        Self::DuplicateEventSink(e.path)
    }
}
