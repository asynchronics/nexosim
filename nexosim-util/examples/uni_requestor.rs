//! Example: sensor reading data from environment model.
//!
//! This example demonstrates in particular:
//!
//! * cyclical self-scheduling methods,
//! * model initialization,
//! * simulation monitoring with buffered event sinks,
//! * connection with mapping,
//! * UniRequestor port.
//!
//! ```text
//!                        ┌─────────────┐               ┌──────────┐
//!                        │             │ temperature   │          │ overheat
//! Temperature ●─────────►│ Environment │◄►───────────►◄│  Sensor  ├──────────►
//!                        │             │               │          │
//!                        └─────────────┘               └──────────┘
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim_util::observable::Observable;

use nexosim::model::{Context, InitializedModel};
use nexosim::ports::{EventQueue, Output, UniRequestor};
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;
use nexosim::{schedulable, Model};

/// Sensor model
#[derive(Serialize, Deserialize)]
pub struct Sensor {
    /// Temperature [deg C] -- requestor port.
    pub temp: UniRequestor<(), f64>,

    /// Overheat detection [-] -- output port.
    pub overheat: Output<bool>,

    /// Temperature threshold [deg C] -- parameter.
    threshold: f64,

    /// Overheat detection [-] -- observable state.
    oh: Observable<bool>,
}

#[Model]
impl Sensor {
    /// Creates new Sensor with overheat threshold set [deg C].
    pub fn new(threshold: f64, temp: UniRequestor<(), f64>) -> Self {
        let overheat = Output::new();
        Self {
            temp,
            overheat: overheat.clone(),
            threshold,
            oh: Observable::with_default(overheat),
        }
    }

    /// Cyclically scheduled method that reads data from environment and
    /// evaluates overheat state.
    #[nexosim(schedulable)]
    pub async fn tick(&mut self) {
        let temp = self.temp.send(()).await;
        if temp > self.threshold {
            if !self.oh.get() {
                self.oh.set(true).await;
            }
        } else if *self.oh.get() {
            self.oh.set(false).await;
        }
    }

    /// Propagate state and schedule cyclic method.
    #[nexosim(init)]
    async fn init(mut self, context: &Context<Self>) -> InitializedModel<Self> {
        self.oh.propagate().await;

        context
            .schedule_periodic_event(
                Duration::from_millis(500),
                Duration::from_millis(500),
                schedulable!(Self::tick),
                (),
            )
            .unwrap();

        self.into()
    }
}

/// Environment model.
#[derive(Serialize, Deserialize)]
pub struct Env {
    /// Temperature [deg F] -- internal state.
    temp: f64,
}

#[Model]
impl Env {
    /// Creates new environment model with the temperature [deg F] set.
    pub fn new(temp: f64) -> Self {
        Self { temp }
    }

    /// Sets temperature [deg F].
    pub async fn set_temp(&mut self, temp: f64) {
        self.temp = temp;
    }

    /// Gets temperature [deg F].
    pub async fn get_temp(&mut self, _: ()) -> f64 {
        self.temp
    }
}

/// Converts Fahrenheit to Celsius.
pub fn fahr_to_cels(t: f64) -> f64 {
    5.0 * (t - 32.0) / 9.0
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Mailboxes.
    let sensor_mbox = Mailbox::new();
    let env_mbox = Mailbox::new();

    // Connect data line and convert Fahrenheit degrees to Celsius.
    let temp_req = UniRequestor::with_map(|x| *x, fahr_to_cels, Env::get_temp, &env_mbox);

    // Models.
    let mut sensor = Sensor::new(100.0, temp_req);
    let env = Env::new(0.0);

    // Model handles for simulation.
    let env_addr = env_mbox.address();

    let overheat = EventQueue::new();
    sensor.overheat.connect_sink(&overheat);
    let mut overheat = overheat.into_reader();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let mut bench = SimInit::new()
        .add_model(sensor, sensor_mbox, "sensor")
        .add_model(env, env_mbox, "env");

    let set_temp_id = bench.register_input(Env::set_temp, env_addr);

    let mut simu = bench.init(t0)?;
    let scheduler = simu.scheduler();

    // ----------
    // Simulation.
    // ----------

    // Check initial conditions.
    assert_eq!(simu.time(), t0);
    assert_eq!(overheat.next(), Some(false));
    assert!(overheat.next().is_none());

    // Change temperature in 2s.
    scheduler
        .schedule_event(Duration::from_secs(2), &set_temp_id, 105.0)
        .unwrap();

    // Change temperature in 4s.
    scheduler
        .schedule_event(Duration::from_secs(4), &set_temp_id, 213.0)
        .unwrap();

    simu.step_until(Duration::from_secs(3))?;
    assert!(overheat.next().is_none());

    simu.step_until(Duration::from_secs(5))?;
    assert_eq!(overheat.next(), Some(true));

    Ok(())
}
