//! Helper models.
//!
//! This module contains helper models useful for simulation bench assembly.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, InitializedModel, InputId, Model, ProtoModel};

/// Ticker model builder.
pub struct ProtoTicker {
    tick: Duration,
}
impl ProtoTicker {
    pub fn new(tick: Duration) -> Self {
        Self { tick }
    }
}
impl ProtoModel for ProtoTicker {
    type Model = Ticker;

    fn build(
        self,
        cx: &mut nexosim::model::BuildContext<Self>,
    ) -> (Self::Model, <Self::Model as Model>::Env) {
        let input_id = cx.register_input(Ticker::tick);
        (Ticker::new(self.tick, input_id), ())
    }
}

/// A ticker model.
///
/// This model self-schedules at the specified period, which can be used to keep
/// the simulation alive.
#[derive(Serialize, Deserialize)]
pub struct Ticker {
    /// Tick period.
    tick: Duration,
    /// Tick InputID
    tick_input_id: InputId<Self, ()>,
}

impl Ticker {
    /// Creates a new `Ticker` with the specified self-scheduling period.
    pub fn new(tick: Duration, tick_input_id: InputId<Self, ()>) -> Self {
        Self {
            tick,
            tick_input_id,
        }
    }

    /// Self-scheduled function.
    async fn tick(&mut self) {}
}

impl Model for Ticker {
    type Env = ();

    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_periodic_event(self.tick, self.tick, &self.tick_input_id, ())
            .unwrap();
        self.into()
    }
}

impl fmt::Debug for Ticker {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Ticker").finish_non_exhaustive()
    }
}
