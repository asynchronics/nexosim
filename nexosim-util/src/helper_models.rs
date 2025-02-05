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
