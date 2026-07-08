//! Example: Servo motor
//!
//!
//! ```text
//!                       ┌─────────────────────────────────────────────────────────────────┐
//!                       │SERVO Motor                                                      │
//!                       │   ┌────────────┐                Voltage                         │
//!                       │   │            │◄────────────────────────────────────────────┐  │
//!                       │   │   Servo    │                            ┌───────────────┐│  │
//!                       │   │ Controller │    Voltage   ┌─────────┐ ┌►│ Potentiometer ├┘  │
//! Set point ●───────────┼──►│            ├─────────────►│         │ │ └───────────────┘   │
//!             (0:180)   │   │            │              │   DC    │ │   position          │
//!                       │   └────────────┘              │  Motor  ├─┴─────────────────────┼──────────►
//!              torque   │                               │         │     (0:180)           │
//!       Load ●──────────┼──────────────────────────────►│         │                       │
//!                       │                               └─────────┘                       │
//!                       └─────────────────────────────────────────────────────────────────┘
//! ```

use std::iter;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, Model, schedulable};
use nexosim::ports::{EventSinkReader, EventSource, Output, SinkState, event_queue};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;

use std::f64::consts::PI;

use utilities::pid::PidController;

const SERVO_MAX: f64 = 180.0;

/// Stepper motor.
#[derive(Serialize, Deserialize)]
pub struct DCMotor {
    /// Position [deg] -- output port.
    pub position: Output<f64>,

    /// Position [deg] -- internal state.
    pos: f64,
    /// Previous velocity [rad/s] -- internal state.
    prev_vel: f64,
    /// time of last position update -- internal state.
    last_position_update: Option<MonotonicTime>,
    /// Torque applied by the load [N·m] -- internal state.
    torque: f64,
}

#[Model]
impl DCMotor {
    /// Creates a new DC motor with position and velocity equal 0
    pub fn new(position: f64) -> Self {
        assert!((0.0..=SERVO_MAX).contains(&position));
        Self {
            position: Default::default(),
            pos: position,
            prev_vel: 0.0,
            last_position_update: None,
            torque: 0.0,
        }
    }

    /// Broadcasts the initial position of the motor.
    #[nexosim(init)]
    async fn init(&mut self) {
        self.position.send(self.pos).await;
    }

    // Motor voltage supplied from controller [V]
    pub async fn voltage_in(&mut self, voltage: f64, cx: &Context<Self>) {
        // real model for velocity to be implemented
        let vel = voltage / 5.0 * PI - self.torque;
        let time = cx.time();
        if let Some(prev_time) = self.last_position_update {
            let elapsed_time = time.duration_since(prev_time).as_secs_f64();
            // Trapezoidal integration of velocity over time with convertion to degrees
            self.pos += (self.prev_vel + vel) / 2.0 * elapsed_time / PI * 180.0;

            self.pos = self.pos.min(SERVO_MAX);
            self.pos = self.pos.max(0.0)
        }
        self.last_position_update = Some(time);
        self.prev_vel = vel;

        self.position.send(self.pos).await;
    }

    /// Torque applied by the load [N·m] -- input port.
    pub fn load(&mut self, torque: f64) {
        self.torque = torque;
    }
}

/// Potentiometer measuring the motor's possition
#[derive(Serialize, Deserialize)]
pub struct Potentiometer {
    /// Voltage corresponding to the position of servo [V] -- output port.
    pub voltage_out: Output<f64>,

    /// Voltage supplied to the servo [V] -- constant.
    supply_voltage: f64,
}

#[Model]
impl Potentiometer {
    /// Creates a new potentiometer setting the supply voltage value
    pub fn new(supply_voltage: f64) -> Self {
        Self {
            voltage_out: Default::default(),
            supply_voltage,
        }
    }

    /// Measures the position of of motor and broadcasts output voltage
    pub async fn measure_position(&mut self, position: f64) {
        self.voltage_out
            .send(position / SERVO_MAX * self.supply_voltage)
            .await;
    }
}

/// motor's controller utilizing PID algorithm
#[derive(Serialize, Deserialize)]
pub struct ServoController {
    /// Voltage controlling the motor [V] -- output port.
    pub voltage_out: Output<f64>,

    /// Position of the motor read from potentiometer [deg] -- internal state.
    pos: f64,
    /// Set point for regulator [deg] -- internal state.
    sp: Option<f64>,
    /// Voltage supplied to the servo [V] -- internal state.
    supply_voltage: f64,
    /// Period of the control loop [s] -- constant.
    period: f64,
    /// Time of previos iteration [-] -- internal state.
    last_control_update: Option<MonotonicTime>,
    /// Value showing if controller had been initialized [-] -- internal state.
    is_idle: bool,
    /// Implementation of PID controller
    pid_impl: PidController,
}

#[Model]
impl ServoController {
    /// Creates a new servo controller
    pub fn new(
        proportional_gain: f64,
        integral_gain: f64,
        derivative_gain: f64,
        supply_voltage: f64,
        period: f64,
    ) -> Self {
        assert!(period > 0.0);
        let pid_impl = PidController::with_limits(
            proportional_gain,
            integral_gain,
            derivative_gain,
            Some(-supply_voltage),
            Some(supply_voltage),
        );
        Self {
            voltage_out: Default::default(),
            pos: 0.0,
            sp: None,
            supply_voltage,
            period,
            last_control_update: None,
            is_idle: true,
            pid_impl,
        }
    }

    /// Reads voltage from the potentiometer
    pub async fn read_position(&mut self, voltage: f64) {
        self.pos = (voltage / self.supply_voltage) * SERVO_MAX;
    }

    /// Sets the set point and shedules first iteration if necessary
    pub async fn set_point(&mut self, value: f64, cx: &Context<Self>) {
        if !(0.0..=SERVO_MAX).contains(&value) {
            return;
        }
        self.sp = Some(value);
        if self.is_idle {
            self.is_idle = false;
            self.set_output((), cx).await;
        }
    }

    /// Sends voltage and schedules next iteration
    #[nexosim(schedulable)]
    async fn set_output(&mut self, _: (), cx: &Context<Self>) {
        let error = self.sp.expect("method won't be used if no setpoint") - self.pos;
        let now = cx.time();
        let dt;
        if let Some(time) = self.last_control_update {
            dt = now.duration_since(time).as_secs_f64();
        } else {
            dt = self.period;
        }
        self.last_control_update = Some(now);
        let voltage = self.pid_impl.update(error, dt);
        self.voltage_out.send(voltage).await;

        let duration = Duration::from_secs_f64(self.period);

        // Schedule the iteration.
        cx.schedule_event(duration, schedulable!(Self::set_output), ())
            .unwrap();
    }
}

fn main() -> Result<(), nexosim::simulation::SimulationError> {
    // Models.
    let sup_voltage = 5.0;
    let period = 0.1;
    let mut motor = DCMotor::new(0.0);
    let mut potentiometer = Potentiometer::new(sup_voltage);
    let mut controller = ServoController::new(0.1, 0.005, 0.01, sup_voltage, period);

    // Mailboxes.
    let motor_mbox = Mailbox::new();
    let potentiometer_mbox = Mailbox::new();
    let controller_mbox = Mailbox::new();

    // Connections
    motor
        .position
        .connect(Potentiometer::measure_position, &potentiometer_mbox);
    potentiometer
        .voltage_out
        .connect(ServoController::read_position, &controller_mbox);
    controller
        .voltage_out
        .connect(DCMotor::voltage_in, &motor_mbox);

    // Endpoints.
    let mut bench = SimInit::new();

    let set_point = EventSource::new()
        .connect(ServoController::set_point, &controller_mbox)
        .register(&mut bench);
    let motor_load = EventSource::new()
        .connect(DCMotor::load, &motor_mbox)
        .register(&mut bench);

    let (sink, mut position) = event_queue(SinkState::Enabled);
    motor.position.connect_sink(sink);

    let t0 = MonotonicTime::EPOCH; // arbitrary since models do not depend on absolute time
    let mut simu = bench
        .add_model(motor, motor_mbox, "motor")
        .add_model(potentiometer, potentiometer_mbox, "potentiometer")
        .add_model(controller, controller_mbox, "controller")
        .init(t0)?;

    // ----------
    // Simulation.
    // ----------

    let scheduler = simu.scheduler();

    // Check initial conditions.
    let t = t0;
    assert_eq!(simu.time(), t);
    assert_eq!(position.try_read(), Some(0.0));
    assert!(position.try_read().is_none());

    // Start the motor in 2s with a PPS of 10Hz.
    scheduler
        .schedule_event(Duration::from_secs(2), &set_point, 90.0)
        .unwrap();

    simu.step_until(Duration::new(6, 0))?;

    simu.process_event(&motor_load, 0.5)?;

    simu.step_until(Duration::new(8, 0))?;

    let positions: Vec<f64> = iter::from_fn(|| position.try_read()).collect();

    println!("Read positions: {}", positions.len());
    dbg!(positions);

    Ok(())
}
