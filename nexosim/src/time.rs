//! Simulation time and clocks.
//!
//! This module provides most notably:
//!
//! * [`MonotonicTime`]: a monotonic timestamp based on the [TAI] time standard,
//! * [`Clock`]: a trait for types that can synchronize a simulation,
//!   implemented for instance by [`SystemClock`] and [`AutoSystemClock`],
//! * [`Ticker`]: a trait to control the responsiveness of simulations,
//!   implemented by [`PeriodicTicker`].
//!
//! [TAI]: https://en.wikipedia.org/wiki/International_Atomic_Time
//!
//!
//! # Examples
//!
//! An alarm clock model that prints a message when the simulation time reaches
//! the specified timestamp.
//!
//! ```
//! use serde::{Deserialize, Serialize};
//! use nexosim::model::{schedulable, Context, Model};
//! use nexosim::time::MonotonicTime;
//!
//! // An alarm clock model.
//! #[derive(Serialize, Deserialize)]
//! pub struct AlarmClock {
//!     msg: String
//! }
//!
//! #[Model]
//! impl AlarmClock {
//!     // Creates a new alarm clock.
//!     pub fn new(msg: String) -> Self {
//!         Self { msg }
//!     }
//!
//!     // Sets an alarm [input port].
//!     pub fn set(&mut self, setting: MonotonicTime, cx: &Context<Self>) {
//!         if cx.schedule_event(setting, schedulable!(Self::ring), ()).is_err() {
//!             println!("The alarm clock can only be set for a future time");
//!         }
//!     }
//!
//!     // Rings the alarm [private input port].
//!     #[nexosim(schedulable)]
//!     fn ring(&mut self) {
//!         println!("{}", self.msg);
//!     }
//! }
//! ```

mod clock;
mod monotonic_time;
mod ticker;

/// A monotonic timestamp with an epoch set at 1970-01-01 00:00:00 TAI.
///
/// `MonotonicTime` is re-exported from crate
/// [`tai_time`](https://docs.rs/tai-time/latest/tai_time/).
/// \
/// \
pub use tai_time::MonotonicTime;

pub use clock::{AutoSystemClock, Clock, NoClock, SyncStatus, SystemClock};
pub use ticker::{PeriodicTicker, Ticker};

pub(crate) use monotonic_time::TearableAtomicTime;

pub(crate) type AtomicTime = crate::util::sync_cell::SyncCell<TearableAtomicTime>;
pub(crate) type AtomicTimeReader = crate::util::sync_cell::SyncCellReader<TearableAtomicTime>;

/// Trait abstracting over time-absolute and time-relative deadlines.
///
/// This trait is implemented by [`std::time::Duration`] and
/// [`MonotonicTime`].
pub trait Deadline {
    /// Make this deadline into an absolute timestamp, using the provided
    /// current time as a reference.
    fn into_time(self, now: MonotonicTime) -> MonotonicTime;
}

impl Deadline for std::time::Duration {
    #[inline(always)]
    fn into_time(self, now: MonotonicTime) -> MonotonicTime {
        now + self
    }
}

impl Deadline for MonotonicTime {
    #[inline(always)]
    fn into_time(self, _: MonotonicTime) -> MonotonicTime {
        self
    }
}

/// A `Clone`-able clock reader to track the current simulation time.
#[derive(Clone)]
pub struct ClockReader(AtomicTimeReader);
impl ClockReader {
    pub(crate) fn from_atomic_time_reader(reader: &AtomicTimeReader) -> Self {
        Self(reader.clone())
    }
    /// Returns the current simulation time.
    ///
    /// Note: it is discouraged to query the time before the simulation is
    /// initialized as the returned value is then meaningless.
    pub fn time(&self) -> MonotonicTime {
        self.0.read()
    }
}
impl std::fmt::Debug for ClockReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClockReader").finish_non_exhaustive()
    }
}

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use std::time::Duration;

    use serde::{Deserialize, Serialize};

    use crate::{
        self as nexosim,
        model::Model,
        ports::EventSource,
        simulation::{Mailbox, SimInit},
    };

    use super::*;

    #[derive(Serialize, Deserialize)]
    struct TestModel;
    #[Model]
    impl TestModel {
        fn input(&mut self) {}
    }

    fn test_clock_reader(num_threads: usize) {
        let model = TestModel;
        let mbox = Mailbox::new();
        let addr = mbox.address();

        let t0 = MonotonicTime::EPOCH;
        let mut bench = SimInit::with_num_threads(num_threads).add_model(model, mbox, "test");

        let event_id = EventSource::new()
            .connect(TestModel::input, &addr)
            .register(&mut bench);
        let reader = bench.clock_reader();

        let mut simu = bench.init(t0).unwrap();
        let scheduler = simu.scheduler();

        scheduler
            .schedule_event(Duration::from_millis(500), &event_id, ())
            .unwrap();

        assert_eq!(reader.time(), simu.time());
        simu.step().unwrap();
        assert_eq!(reader.time(), simu.time());
    }

    #[test]
    fn clock_reader_st() {
        test_clock_reader(1);
    }

    #[test]
    fn clock_reader_mt() {
        test_clock_reader(4);
    }
}
