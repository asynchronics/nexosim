//! Helper models.
//!
//! This module contains helper models useful for simulation bench assembly.
//!

use std::fmt;
use std::time::Duration;

use nexosim::model::{Context, InitializedModel, Model};

/// A ticker model.
///
/// This model self-schedules at the specified period, which can be used to keep
/// the simulation alive.
///
/// An example below shows adding a ticker model and making simulation run
/// infinitely until some condition is met.
///
/// ```rust
/// use std::thread;
/// use std::time::Duration;
///
/// use nexosim::simulation::{ExecutionError, Mailbox, SimInit};
/// use nexosim::time::{AutoSystemClock, MonotonicTime};
///
/// use nexosim_util::models::Ticker;
///
/// const TICK: Duration = Duration::from_millis(100);
///
/// // The ticker model that keeps simulation alive.
/// let ticker = Ticker::new(TICK);
/// let ticker_mbox = Mailbox::new();
///
/// // Start time (arbitrary since models do not depend on absolute time).
/// let t0 = MonotonicTime::EPOCH;
///
/// // Assembly and initialization.
/// let (mut simu, mut scheduler) = SimInit::new()
///    .add_model(ticker, ticker_mbox, "ticker")
///    .set_clock(AutoSystemClock::new())
///    .init(t0).unwrap();
///
/// // Simulation thread.
/// let simulation_handle = thread::spawn(move || {
///     //---------- Simulation.  ----------
///     //Infinitely kept alive by the ticker model until halted.
///     simu.step_unbounded()
/// });
///
/// // Do some job and wait for some condition.
///
/// scheduler.halt();
/// assert!(matches!(simulation_handle.join().unwrap(),
///     Err(ExecutionError::Halted)));
/// ```
pub struct Ticker {
    /// Tick period.
    tick: Duration,
}

impl Ticker {
    /// Creates a new `Ticker` with the specified self-scheduling period.
    pub fn new(tick: Duration) -> Self {
        Self { tick }
    }

    /// Self-scheduled function.
    async fn tick(&mut self) {}
}

impl Model for Ticker {
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_periodic_event(self.tick, self.tick, Self::tick, ())
            .unwrap();
        self.into()
    }
}

impl fmt::Debug for Ticker {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Ticker").finish_non_exhaustive()
    }
}
