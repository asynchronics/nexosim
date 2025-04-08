//! Example: power supply with parallel resistive loads.
//!
//! This example demonstrates in particular:
//!
//! * the use of requestor and replier ports,
//! * simulation monitoring with event slots.
//!
//! ```text
//!                                                     ┌────────┐
//!                                                     │        │
//!                                                ┌──◄►│  Load  ├───► Power
//!                                                │    │        │
//!                                                │    └────────┘
//!                                                │
//!                                                │    ┌────────┐
//!                                                │    │        │
//!                                                ├──◄►│  Load  ├───► Power
//!                                                │    │        │
//!                                                │    └────────┘
//!                                                │
//!                                                │    ┌────────┐
//!                       ┌──────────┐   voltage►  │    │        │
//! Voltage setting ●────►│          │►◄───────────┴──◄►│  Load  ├───► Power
//!                       │  Power   │  ◄current        │        │
//!                       │  supply  │                  └────────┘
//!                       │          ├───────────────────────────────► Total power
//!                       └──────────┘
//! ```

use serde::{Deserialize, Serialize};

use nexosim::model::{Environment, Model};
use nexosim::ports::{EventSlot, Output, Requestor};
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;

#[derive(Default)]
pub struct PowerSupplyEnv {
    /// Electrical output [V → A] -- requestor port.
    pub pwr_out: Requestor<f64, f64>,
    /// Power consumption [W] -- output port.
    pub power: Output<f64>,
}
impl Environment for PowerSupplyEnv {}

/// Power supply.
#[derive(Serialize, Deserialize)]
pub struct PowerSupply;

impl PowerSupply {
    /// Voltage setting [V] -- input port.
    pub async fn voltage_setting(&mut self, voltage: f64, env: &mut PowerSupplyEnv) {
        // Ignore negative values.
        if voltage < 0.0 {
            return;
        }

        // Sum all load currents.
        let mut total_current = 0.0;
        for current in env.pwr_out.send(voltage).await {
            total_current += current;
        }

        env.power.send(voltage * total_current).await;
    }
}

impl Model for PowerSupply {
    type Environment = PowerSupplyEnv;
}

#[derive(Default)]
pub struct LoadEnv {
    /// Power consumption [W] -- output port.
    pub power: Output<f64>,
}
impl Environment for LoadEnv {}

/// Power supply.
#[derive(Serialize, Deserialize)]
pub struct Load {
    /// Load conductance [S] -- internal state.
    conductance: f64,
}

impl Load {
    /// Creates a load with the specified resistance [Ω].
    fn new(resistance: f64) -> Self {
        assert!(resistance > 0.0);
        Self {
            conductance: 1.0 / resistance,
        }
    }

    /// Electrical input [V → A] -- replier port.
    ///
    /// This port receives the applied voltage and returns the load current.
    pub async fn pwr_in(&mut self, voltage: f64, env: &mut LoadEnv) -> f64 {
        let current = voltage * self.conductance;
        env.power.send(voltage * current).await;

        current
    }
}

impl Model for Load {
    type Environment = LoadEnv;
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.
    let r1 = 5.0;
    let r2 = 10.0;
    let r3 = 20.0;
    let mut psu = PowerSupply;
    let mut load1 = Load::new(r1);
    let mut load2 = Load::new(r2);
    let mut load3 = Load::new(r3);

    // Environments
    let mut psu_env = PowerSupplyEnv::default();
    let mut load1_env = LoadEnv::default();
    let mut load2_env = LoadEnv::default();
    let mut load3_env = LoadEnv::default();

    // Mailboxes.
    let psu_mbox = Mailbox::new();
    let load1_mbox = Mailbox::new();
    let load2_mbox = Mailbox::new();
    let load3_mbox = Mailbox::new();

    // Connections.
    psu_env.pwr_out.connect(Load::pwr_in, &load1_mbox);
    psu_env.pwr_out.connect(Load::pwr_in, &load2_mbox);
    psu_env.pwr_out.connect(Load::pwr_in, &load3_mbox);

    // Model handles for simulation.
    let mut psu_power = EventSlot::new();
    let mut load1_power = EventSlot::new();
    let mut load2_power = EventSlot::new();
    let mut load3_power = EventSlot::new();
    psu_env.power.connect_sink(&psu_power);
    load1_env.power.connect_sink(&load1_power);
    load2_env.power.connect_sink(&load2_power);
    load3_env.power.connect_sink(&load3_power);
    let psu_addr = psu_mbox.address();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let mut simu = SimInit::new()
        .add_model(psu, psu_env, psu_mbox, "psu")
        .add_model(load1, load1_env, load1_mbox, "load1")
        .add_model(load2, load2_env, load2_mbox, "load2")
        .add_model(load3, load3_env, load3_mbox, "load3")
        .init(t0)?
        .0;

    // ----------
    // Simulation.
    // ----------

    // Compare two electrical powers for equality [W].
    fn same_power(a: f64, b: f64) -> bool {
        // Use an absolute floating-point epsilon of 1 pW.
        (a - b).abs() < 1e-12
    }

    // Vary the supply voltage, check the load and power supply consumptions.
    for voltage in [10.0, 15.0, 20.0] {
        simu.process_event(PowerSupply::voltage_setting, voltage, &psu_addr)?;

        let v_square = voltage * voltage;
        assert!(same_power(load1_power.next().unwrap(), v_square / r1));
        assert!(same_power(load2_power.next().unwrap(), v_square / r2));
        assert!(same_power(load3_power.next().unwrap(), v_square / r3));
        assert!(same_power(
            psu_power.next().unwrap(),
            v_square * (1.0 / r1 + 1.0 / r2 + 1.0 / r3)
        ));
    }

    Ok(())
}
