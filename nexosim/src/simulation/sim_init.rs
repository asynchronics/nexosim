use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Serialize, de::DeserializeOwned};

use crate::channel::ChannelObserver;
use crate::endpoints::{
    Endpoints, EventSinkInfoRegistry, EventSinkRegistry, EventSourceRegistry, QuerySourceRegistry,
};
use crate::executor::{Executor, SimulationContext};
use crate::model::{Message, ProtoModel, RegisteredModel};
use crate::ports::{EventSinkReader, EventSource, QuerySource};
use crate::time::{
    AtomicTime, Clock, ClockReader, MonotonicTime, NoClock, SyncStatus, TearableAtomicTime,
};
use crate::util::sync_cell::SyncCell;

use super::{
    EventId, ExecutionError, GlobalScheduler, Mailbox, QueryId, SchedulerQueue, SchedulerRegistry,
    Signal, Simulation, SimulationError, add_model,
};

type PostCallback = dyn FnMut(&mut Simulation) -> Result<(), SimulationError> + 'static;

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
        callback: impl FnMut(&mut Simulation) -> Result<(), SimulationError> + 'static,
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
        callback: impl FnMut(&mut Simulation) -> Result<(), SimulationError> + 'static,
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
    /// If the specified name is already used by another input or another event
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`].
    pub(crate) fn add_event_source<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), DuplicateEventSourceError<T>>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry
            .add(source, name.into(), &mut self.scheduler_registry)
            .map_err(|(name, source)| DuplicateEventSourceError { name, source })
    }

    /// Adds an event source to the endpoint registry without requiring a
    /// [`Message`] implementation for its item type.
    ///
    /// If the specified name is already used by another input or another event
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`].
    pub(crate) fn add_event_source_raw<T>(
        &mut self,
        source: EventSource<T>,
        name: impl Into<String>,
    ) -> Result<(), DuplicateEventSourceError<T>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
    {
        self.event_source_registry
            .add_raw(source, name.into(), &mut self.scheduler_registry)
            .map_err(|(name, source)| DuplicateEventSourceError { name, source })
    }

    /// Adds a query source to the endpoint registry.
    ///
    /// If the specified name is already used by another query
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`].
    pub(crate) fn add_query_source<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), DuplicateQuerySourceError<QuerySource<T, R>>>
    where
        T: Message + Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Message + Serialize + Send + 'static,
    {
        self.query_source_registry
            .add(source, name.into(), &mut self.scheduler_registry)
            .map_err(|(name, source)| DuplicateQuerySourceError { name, source })
    }

    /// Adds a query source to the endpoint registry without requiring
    /// [`Message`] implementations for its query and response types.
    ///
    /// If the specified name is already used by another query
    /// source, the source provided as argument is returned in the error. The
    /// error is convertible to an [`InitError`].
    pub(crate) fn add_query_source_raw<T, R>(
        &mut self,
        source: QuerySource<T, R>,
        name: impl Into<String>,
    ) -> Result<(), DuplicateQuerySourceError<QuerySource<T, R>>>
    where
        T: Serialize + DeserializeOwned + Clone + Send + 'static,
        R: Serialize + Send + 'static,
    {
        self.query_source_registry
            .add_raw(source, name.into(), &mut self.scheduler_registry)
            .map_err(|(name, source)| DuplicateQuerySourceError { name, source })
    }

    /// Adds an event sink to the endpoint registry.
    ///
    /// If the specified name is already used by another event sink, the event
    /// sink provided as argument is returned in the error.
    pub fn add_event_sink<S, T>(
        mut self,
        sink: S,
        name: impl Into<String>,
    ) -> Result<Self, DuplicateEventSinkError<S>>
    where
        S: EventSinkReader<T> + Send + Sync + 'static,
        S::Item: Message + Serialize,
        T: 'static,
    {
        let name = name.into();

        if self
            .event_sink_info_registry
            .register::<T>(name.clone())
            .is_err()
        {
            return Err(DuplicateEventSinkError { name, sink });
        };

        self.event_sink_registry
            .add(sink, name)
            .map_err(|(name, sink)| DuplicateEventSinkError { name, sink })?;

        Ok(self)
    }

    /// Adds an event sink to the endpoint registry without requiring a
    /// [`Message`] implementation for its item type.
    ///
    /// If the specified name is already used by another event sink, the event
    /// sink provided as argument is returned in the error.
    pub fn add_event_sink_raw<S, T>(
        mut self,
        sink: S,
        name: impl Into<String>,
    ) -> Result<Self, DuplicateEventSinkError<S>>
    where
        S: EventSinkReader<T> + Send + Sync + 'static,
        S::Item: Serialize,
        T: 'static,
    {
        let name = name.into();

        if self
            .event_sink_info_registry
            .register_raw(name.clone())
            .is_err()
        {
            return Err(DuplicateEventSinkError { name, sink });
        };

        self.event_sink_registry
            .add(sink, name)
            .map_err(|(name, sink)| DuplicateEventSinkError { name, sink })?;

        Ok(self)
    }

    fn build(self) -> (Simulation, Endpoints) {
        let simulation = Simulation::new(
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
        );
        let endpoint_registry = Endpoints::new(
            self.event_sink_registry,
            self.event_sink_info_registry,
            self.event_source_registry,
            self.query_source_registry,
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
    ) -> Result<(Simulation, Endpoints), SimulationError> {
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
    ) -> Result<(Simulation, Endpoints), SimulationError> {
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

/// Error returned when the initialization of a simulation fails.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum InitError {
    /// An attempt to add an input or event source failed because the provided
    /// name is already in use.
    DuplicateEventName(String),
    /// An attempt to add a query source failed because the provided name is
    /// already in use.
    DuplicateQueryName(String),
    /// An attempt to add an event sink failed because the provided name is
    /// already in use.
    DuplicateSinkName(String),
}

impl fmt::Display for InitError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateEventName(name) => write!(
                fmt,
                "cannot add input or event source under name `{name}` as this name is already in use",
            ),
            Self::DuplicateQueryName(name) => write!(
                fmt,
                "cannot add query source under name `{name}` as this name is already in use",
            ),
            Self::DuplicateSinkName(name) => write!(
                fmt,
                "cannot add event sink under name `{name}` as this name is already in use",
            ),
        }
    }
}
impl Error for InitError {}

/// Error returned when attempting to add an event source with an existing name.
pub struct DuplicateEventSourceError<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    /// Name of the event source.
    pub name: String,
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
            "cannot add event source under name `{}` as this name is already in use",
            self.name
        )
    }
}
impl<T> fmt::Debug for DuplicateEventSourceError<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateEventSource")
            .field("name", &self.name)
            .field("source", &"<EventSource<T>>")
            .finish()
    }
}
impl<T> Error for DuplicateEventSourceError<T> where
    T: Serialize + DeserializeOwned + Clone + Send + 'static
{
}

impl<T: Serialize + DeserializeOwned + Clone + Send + 'static> From<DuplicateEventSourceError<T>>
    for InitError
{
    fn from(e: DuplicateEventSourceError<T>) -> Self {
        Self::DuplicateEventName(e.name)
    }
}

/// Error returned when attempting to add an input with an existing name.
#[derive(Debug)]
pub struct DuplicateInputError {
    /// Name of the input.
    pub name: String,
}
impl fmt::Display for DuplicateInputError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "cannot add input under name `{}` as this name is already in use",
            self.name
        )
    }
}
impl Error for DuplicateInputError {}

impl From<DuplicateInputError> for InitError {
    fn from(e: DuplicateInputError) -> Self {
        Self::DuplicateEventName(e.name)
    }
}

/// Error returned when attempting to add a query source with an existing name.
pub struct DuplicateQuerySourceError<S> {
    /// Name of the query source.
    pub name: String,
    /// The query source.
    pub source: S,
}
impl<S> fmt::Display for DuplicateQuerySourceError<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "cannot add query source under name `{}` as this name is already in use",
            self.name
        )
    }
}
impl<S> fmt::Debug for DuplicateQuerySourceError<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateQuerySource")
            .field("name", &self.name)
            .field("source", &"<QuerySource<T, R>>")
            .finish()
    }
}
impl<S> Error for DuplicateQuerySourceError<S> {}

impl<S> From<DuplicateQuerySourceError<S>> for InitError {
    fn from(e: DuplicateQuerySourceError<S>) -> Self {
        Self::DuplicateQueryName(e.name)
    }
}

/// Error returned when attempting to add an event sink with an existing name.
pub struct DuplicateEventSinkError<S> {
    /// Name of the event sink.
    pub name: String,
    /// The event sink.
    pub sink: S,
}
impl<S> fmt::Display for DuplicateEventSinkError<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "cannot add event sink under name `{}` as this name is already in use",
            self.name
        )
    }
}
impl<S> fmt::Debug for DuplicateEventSinkError<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateEventSource")
            .field("name", &self.name)
            .field("sink", &"<EventSink>")
            .finish()
    }
}
impl<S> Error for DuplicateEventSinkError<S> {}

impl<S> From<DuplicateEventSinkError<S>> for InitError {
    fn from(e: DuplicateEventSinkError<S>) -> Self {
        Self::DuplicateSinkName(e.name)
    }
}
