//! Example: elevator with a motor-driven pulley, flexible cable, and car.
//!
//! This example demonstrates:
//!
//! * modeling a third-order translational mechanical system,
//! * a pulley/cable coupling between rotary and linear motion,
//! * gravity as a constant disturbance.
//!
//!
//! ```text
//!           ┌───────────────────────────────────────┐
//!  Motor    │  Elevator                             │
//!  torque   │    ┌─────────┐    cable    ┌─────┐   │► car velocity [m/s]
//!  [N·m] ──►│    │ pulley  │◄───────────►│ car │   │► car position [m]
//!           │    └─────────┘             └─────┘   │
//!           └───────────────────────────────────────┘
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, Model, schedulable};
use nexosim::ports::{EventSinkReader, EventSource, Output, SinkState, event_slot};
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;

/// Acceleration due to gravity [m/s²].
const G: f64 = 9.81;

/// Elevator model: motor, pulley, cable, and car.
#[derive(Serialize, Deserialize)]
pub struct Elevator {
    /// Car velocity [m/s] -- output port.
    pub car_velocity: Output<f64>,
    /// Car position [m] -- output port.
    pub car_position: Output<f64>,

    // Parameters.
    /// Pulley moment of inertia [kg·m²].
    pulley_inertia: f64,
    /// Pulley radius [m].
    pulley_radius: f64,
    /// Cable stiffness [N/m].
    cable_stiffness: f64,
    /// Car mass (with occupants) [kg].
    car_mass: f64,
    /// Linear damping coefficient at the pulley bearing [N·m·s/rad].
    damping: f64,

    // State.
    /// Commanded motor torque [N·m].
    commanded_torque: f64,
    /// Pulley angular speed [rad/s].
    pulley_speed: f64,
    /// Cable tension force [N].
    cable_force: f64,
    /// Car velocity [m/s].
    velocity: f64,
    /// Car position [m].
    position: f64,
    /// Time of the last integration step.
    last_update: Option<MonotonicTime>,
}

#[Model]
impl Elevator {
    /// Integration period.
    const PERIOD: Duration = Duration::from_millis(1);

    /// Creates a new elevator model.
    pub fn new(
        pulley_inertia: f64,
        pulley_radius: f64,
        cable_stiffness: f64,
        car_mass: f64,
        damping: f64,
    ) -> Self {
        Self {
            car_velocity: Default::default(),
            car_position: Default::default(),
            pulley_inertia,
            pulley_radius,
            cable_stiffness,
            car_mass,
            damping,
            commanded_torque: 0.0,
            pulley_speed: 0.0,
            cable_force: 0.0,
            velocity: 0.0,
            position: 0.0,
            last_update: None,
        }
    }

    /// Schedules periodic integration and broadcasts the initial outputs.
    #[nexosim(init)]
    async fn init(&mut self, cx: &Context<Self>) {
        cx.schedule_periodic_event(
            Self::PERIOD,
            Self::PERIOD,
            schedulable!(Self::integrate),
            (),
        )
        .unwrap();

        self.car_velocity.send(self.velocity).await;
        self.car_position.send(self.position).await;
    }

    /// Motor torque command [N·m] -- input port.
    pub fn motor_torque(&mut self, torque: f64) {
        self.commanded_torque = torque;
    }

    /// Integrates the state-space equations and broadcasts the car outputs.
    #[nexosim(schedulable)]
    async fn integrate(&mut self, _: (), cx: &Context<Self>) {
        let now = cx.time();

        if let Some(prev) = self.last_update {
            let dt = now.duration_since(prev).as_secs_f64();

            // State equations from de Silva, Example 3.3.
            //   J·ω̇ = Tm − r·f − b·ω
            //   ḟ  = k·(r·ω − v)
            //   m·v̇ = f − m·g
            let torque_damping = self.damping * self.pulley_speed;
            let speed_dot =
                (self.commanded_torque - self.pulley_radius * self.cable_force - torque_damping)
                    / self.pulley_inertia;
            let force_dot =
                self.cable_stiffness * (self.pulley_radius * self.pulley_speed - self.velocity);
            let velocity_dot = (self.cable_force - self.car_mass * G) / self.car_mass;

            self.pulley_speed += speed_dot * dt;
            self.cable_force += force_dot * dt;
            self.velocity += velocity_dot * dt;
            self.position += self.velocity * dt;
        }

        self.last_update = Some(now);
        self.car_velocity.send(self.velocity).await;
        self.car_position.send(self.position).await;
    }
}

pub fn main() -> Result<(), SimulationError> {
    // System parameters.
    const PULLEY_INERTIA: f64 = 0.5; // kg·m²
    const PULLEY_RADIUS: f64 = 0.25; // m
    const CABLE_STIFFNESS: f64 = 10_000.0; // N/m
    const CAR_MASS: f64 = 200.0; // kg
    const DAMPING: f64 = 20.0; // N·m·s/rad

    let mut elevator = Elevator::new(
        PULLEY_INERTIA,
        PULLEY_RADIUS,
        CABLE_STIFFNESS,
        CAR_MASS,
        DAMPING,
    );
    let elevator_mbox = Mailbox::new();

    let mut bench = SimInit::new();

    let torque_cmd = EventSource::new()
        .connect(Elevator::motor_torque, &elevator_mbox)
        .register(&mut bench);

    let (vel_sink, mut velocity) = event_slot(SinkState::Enabled);
    elevator.car_velocity.connect_sink(vel_sink);

    let (pos_sink, mut position) = event_slot(SinkState::Enabled);
    elevator.car_position.connect_sink(pos_sink);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench
        .add_model(elevator, elevator_mbox, "elevator")
        .init(t0)?;

    // Apply a step motor torque large enough to lift the car.
    simu.process_event(&torque_cmd, 600.0)?;

    // Run for 3 seconds.
    simu.step_until(Duration::from_secs(3))?;

    let v = velocity.try_read().expect("velocity should be available");
    let p = position.try_read().expect("position should be available");
    println!("Final car velocity: {v:.3} m/s, position: {p:.3} m");

    assert!(v.is_finite());
    assert!(p.is_finite());
    assert!(p > 0.0);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper that builds a bench, applies a step torque, runs for `dt`,
    /// and returns the final car velocity and position.
    fn run(torque: f64, dt: Duration, name: &str) -> Result<(f64, f64), SimulationError> {
        let mut elevator = Elevator::new(0.5, 0.25, 10_000.0, 200.0, 20.0);
        let elevator_mbox = Mailbox::new();

        let mut bench = SimInit::new();

        let torque_cmd = EventSource::new()
            .connect(Elevator::motor_torque, &elevator_mbox)
            .register(&mut bench);

        let (vel_sink, mut velocity) = event_slot(SinkState::Enabled);
        elevator.car_velocity.connect_sink(vel_sink);

        let (pos_sink, mut position) = event_slot(SinkState::Enabled);
        elevator.car_position.connect_sink(pos_sink);

        let mut simu = bench
            .add_model(elevator, elevator_mbox, name)
            .init(MonotonicTime::EPOCH)?;

        simu.process_event(&torque_cmd, torque)?;
        simu.step_until(dt)?;

        let v = velocity.try_read().expect("velocity should be available");
        let p = position.try_read().expect("position should be available");
        Ok((v, p))
    }

    #[test]
    fn command_is_stored() {
        let mut elevator = Elevator::new(0.5, 0.25, 10_000.0, 200.0, 20.0);
        elevator.motor_torque(450.0);
        assert_eq!(elevator.commanded_torque, 450.0);
    }

    #[test]
    fn initial_state_is_zero() {
        let elevator = Elevator::new(0.5, 0.25, 10_000.0, 200.0, 20.0);
        assert_eq!(elevator.commanded_torque, 0.0);
        assert_eq!(elevator.pulley_speed, 0.0);
        assert_eq!(elevator.cable_force, 0.0);
        assert_eq!(elevator.velocity, 0.0);
        assert_eq!(elevator.position, 0.0);
        assert!(elevator.last_update.is_none());
    }

    #[test]
    fn car_falls_when_motor_is_off() -> Result<(), SimulationError> {
        let (v, p) = run(0.0, Duration::from_secs(2), "elevator_off")?;
        assert!(v.is_finite());
        assert!(p.is_finite());
        assert!(v < 0.0, "car should move downward under gravity");
        assert!(p < 0.0, "position should decrease under gravity");
        Ok(())
    }

    #[test]
    fn car_rises_with_sufficient_torque() -> Result<(), SimulationError> {
        let (v, p) = run(600.0, Duration::from_secs(2), "elevator_up")?;
        assert!(v.is_finite());
        assert!(p.is_finite());
        assert!(v > 0.0, "car should move upward");
        assert!(p > 0.0, "position should increase");
        Ok(())
    }
}
