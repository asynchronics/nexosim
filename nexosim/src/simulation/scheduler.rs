//! Scheduling functions and types.
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::simulation::events::{Event, EventKey, SourceId};
use crate::time::{AtomicTimeReader, ClockReader, Deadline, MonotonicTime};
use crate::util::priority_queue::PriorityQueue;

#[cfg(all(test, not(nexosim_loom)))]
use crate::{time::TearableAtomicTime, util::sync_cell::SyncCell};

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
    /// If multiple sources send events at the same simulation time to the same
    /// model, these events are guaranteed to be processed according to the
    /// scheduling order.
    #[cfg(feature = "server")]
    pub(crate) fn schedule(
        &self,
        deadline: impl Deadline,
        event: Event,
    ) -> Result<(), SchedulingError> {
        self.0
            .schedule_from(deadline, event, GLOBAL_SCHEDULER_ORIGIN_ID)
    }

    /// Schedules an event by its id at a future time.
    ///
    /// An error is returned if the specified time is not in the future of the
    /// current simulation time.
    ///
    /// Events scheduled for the same time and targeting the same model are
    /// guaranteed to be processed according to the scheduling order.
    pub fn schedule_event<T>(
        &self,
        deadline: impl Deadline,
        source_id: &SourceId<T>,
        arg: T,
    ) -> Result<(), SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        self.0
            .schedule_event_from(deadline, source_id, arg, GLOBAL_SCHEDULER_ORIGIN_ID)
    }

    /// Schedules a cancellable event by its id at a future time and returns an
    /// event key.
    ///
    /// An error is returned if the specified time is not in the future of the
    /// current simulation time.
    ///
    /// Events scheduled for the same time and targeting the same model are
    /// guaranteed to be processed according to the scheduling order.
    pub fn schedule_keyed_event<T>(
        &self,
        deadline: impl Deadline,
        source_id: &SourceId<T>,
        arg: T,
    ) -> Result<EventKey, SchedulingError>
    where
        T: Send + Clone + 'static,
    {
        self.0
            .schedule_keyed_event_from(deadline, source_id, arg, GLOBAL_SCHEDULER_ORIGIN_ID)
    }

    /// Schedules a periodically recurring event by its id at a future time.
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
        source_id: &SourceId<T>,
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

    /// Schedules a cancellable, periodically recurring event by its id at a
    /// future time and returns an event key.
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
        source_id: &SourceId<T>,
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
#[non_exhaustive]
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

/// Alias for the scheduler queue type.
///
/// Why use both time and origin ID as the key? The short answer is that this
/// allows to preserve the relative ordering of events which have the same
/// origin (where the origin is either a model instance or the global
/// scheduler). The preservation of this ordering is implemented by the event
/// loop, which aggregate events with the same origin into single sequential
/// futures, thus ensuring that they are not executed concurrently.
pub(crate) type SchedulerQueue = PriorityQueue<SchedulerKey, Event>;

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

    pub(crate) fn clock_reader(&self) -> ClockReader {
        ClockReader::from_atomic_time_reader(&self.time)
    }

    /// Schedules an event identified by its origin at a future time.
    #[cfg(feature = "server")]
    pub(crate) fn schedule_from(
        &self,
        deadline: impl Deadline,
        event: Event,
        origin_id: usize,
    ) -> Result<(), SchedulingError> {
        // The scheduler queue must always be locked when reading the time,
        // otherwise the following race could occur:
        // 1) this method reads the time and concludes that it is not too late to
        //    schedule the action,
        // 2) the `Simulation` object takes the lock, increments simulation time and
        //    runs the simulation step,
        // 3) this method takes the lock and schedules the now-outdated action.
        let mut scheduler_queue = self.scheduler_queue.lock().unwrap();

        let now = self.time();
        let time = deadline.into_time(now);
        if now >= time {
            return Err(SchedulingError::InvalidScheduledTime);
        }

        scheduler_queue.insert((time, origin_id), event);

        Ok(())
    }

    /// Schedules an event identified by its id and origin at a future time.
    pub(crate) fn schedule_event_from<T>(
        &self,
        deadline: impl Deadline,
        source_id: &SourceId<T>,
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

        let event = Event::new(source_id, arg);
        scheduler_queue.insert((time, origin_id), event);

        Ok(())
    }

    /// Schedules a cancellable event identified by its id and origin at a
    /// future time and returns an event key.
    pub(crate) fn schedule_keyed_event_from<T>(
        &self,
        deadline: impl Deadline,
        source_id: &SourceId<T>,
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

        let event = Event::new(source_id, arg).with_key(event_key.clone());
        scheduler_queue.insert((time, origin_id), event);

        Ok(event_key)
    }

    /// Schedules a periodically recurring event identified by its id and origin
    /// at a future time.
    pub(crate) fn schedule_periodic_event_from<T>(
        &self,
        deadline: impl Deadline,
        period: Duration,
        source_id: &SourceId<T>,
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

        let event = Event::new(source_id, arg).with_period(period);
        scheduler_queue.insert((time, origin_id), event);

        Ok(())
    }

    /// Schedules a cancellable, periodically recurring event identified by its
    /// id and origin at a future time and returns an event key.
    pub(crate) fn schedule_keyed_periodic_event_from<T>(
        &self,
        deadline: impl Deadline,
        period: Duration,
        source_id: &SourceId<T>,
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

        let event = Event::new(source_id, arg)
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
