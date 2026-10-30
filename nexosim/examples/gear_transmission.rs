//! Example: rotary motion controller with gear transmission.
//!
//! This example demonstrates:
//!
//! * modeling a second-order rotational mechanical system,
//! * a gear transmission as a speed/torque transformer,
//! * state-space integration inside a simulation model.
//!
//! It is based on Example 4.6 in Clarence W. de Silva,
//! *Modeling and Control of Engineering Systems*.
//!
//! ```text
//!                     ┌─────────────────────────────┐
//!                     │  GearTransmission           │
//!  Velocity command   │    ┌───────────┐            │
//!  [rad/s] ●─────────►│    │  Motor &  │ position   │►
//!                     │    │  Load     │ [rad]      │
//!                     │    └───────────┘            │
//!                     └─────────────────────────────┘
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, Model, schedulable};
use nexosim::ports::{EventSinkReader, Output, SinkState, event_queue};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;

/// Rotary-motion system with a gear transmission.
#[derive(Serialize, Deserialize)]
pub struct GearTransmission {
    /// Load angular position [rad] -- output port.
    pub position: Output<f64>,

    // Parameters.
    /// Motor inertia [kg·m²].
    motor_inertia: f64,
    /// Load inertia [kg·m²].
    load_inertia: f64,
    /// Rotary damping [N·m·s/rad].
    damping: f64,
    /// Torsional stiffness [N·m/rad].
    stiffness: f64,
    /// Gear speed ratio: load speed / motor speed.
    speed_ratio: f64,

    // State.
    /// Commanded motor speed [rad/s].
    command: f64,
    /// Motor angular speed [rad/s].
    motor_speed: f64,
    /// Spring torque [N·m].
    spring_torque: f64,
    /// Load angular position [rad].
    load_position: f64,
    /// Time of the last integration step.
    last_update: Option<MonotonicTime>,
}

#[Model]
impl GearTransmission {
    /// Integration period.
    const PERIOD: Duration = Duration::from_millis(1);

    /// Creates a new gear transmission system.
    pub fn new(
        motor_inertia: f64,
        load_inertia: f64,
        damping: f64,
        stiffness: f64,
        speed_ratio: f64,
    ) -> Self {
        Self {
            position: Default::default(),
            motor_inertia,
            load_inertia,
            damping,
            stiffness,
            speed_ratio,
            command: 0.0,
            motor_speed: 0.0,
            spring_torque: 0.0,
            load_position: 0.0,
            last_update: None,
        }
    }

    /// Broadcasts the initial position.
    #[nexosim(init)]
    async fn init(&mut self, cx: &Context<Self>) {
        cx.schedule_periodic_event(
            Self::PERIOD,
            Self::PERIOD,
            schedulable!(Self::integrate),
            (),
        )
        .unwrap();

        self.position.send(self.load_position).await;
    }

    /// Velocity command [rad/s] -- input port.
    pub fn velocity_command(&mut self, command: f64) {
        self.command = command;
    }

    /// Integrates the state-space equations and broadcasts the load position.
    #[nexosim(schedulable)]
    async fn integrate(&mut self, _: (), cx: &Context<Self>) {
        // TODO: implement ODE integration.
        self.position.send(self.load_position).await;
    }
}

#[allow(dead_code)]
fn main() -> Result<(), nexosim::simulation::SimulationError> {
    Ok(())
}
