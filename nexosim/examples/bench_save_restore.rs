//! Example: espresso coffee machine with the bench API.
//!
//! This example demonstrates in particular:
//!
//! * the creation of a bench that can be used e.g. from a remote gRPC client
//!   (remote client use not shown here),
//! * simulation state save and restore functionality.
//!
//! ```text
//!                                                   flow rate
//!                                ┌─────────────────────────────────────────────┐
//!                                │                     (≥0)                    │
//!                                │    ┌────────────┐                           │
//!                                └───►│            │                           │
//!                   added volume      │ Water tank ├────┐                      │
//!     Water fill ●───────────────────►│            │    │                      │
//!                      (>0)           └────────────┘    │                      │
//!                                                       │                      │
//!                                      water sense      │                      │
//!                                ┌──────────────────────┘                      │
//!                                │  (empty|not empty)                          │
//!                                │                                             │
//!                                │    ┌────────────┐          ┌────────────┐   │
//!                    brew time   └───►│            │ command  │            │   │
//! Brew time dial ●───────────────────►│ Controller ├─────────►│ Water pump ├───┘
//!                      (>0)      ┌───►│            │ (on|off) │            │
//!                                │    └────────────┘          └────────────┘
//!                    trigger     │
//!   Brew command ●───────────────┘
//!                      (-)
//! ```

use std::time::Duration;

use nexosim::endpoints::Endpoints;
use nexosim::ports::{EventQueue, EventSource, QuerySource};
use nexosim::simulation::{
    EventId, InitError, Mailbox, QueryId, SimInit, Simulation, SimulationError,
};
use nexosim::time::MonotonicTime;

mod espresso_machine;

pub use espresso_machine::{Controller, Pump, Tank};

/// The constant mass flow rate assumption is of course a gross
/// simplification, so the flow rate is set to an expected average over the
/// whole extraction [m³·s⁻¹].
const PUMP_FLOW_RATE: f64 = 4.5e-6;
/// Start with 1.5l in the tank [m³].
const INIT_TANK_VOLUME: f64 = 1.5e-3;

/// Build a simulation bench using the bench API.
///
/// The same function could be used to build a gRPC server (see the `server`
/// feature) and manage the simulation from a gRPC Python client.
pub fn build_bench((pump_flow_rate, init_tank_volume): (f64, f64)) -> Result<SimInit, InitError> {
    // Models.
    let mut pump = Pump::new(pump_flow_rate);
    let mut controller = Controller::new();
    let mut tank = Tank::new(init_tank_volume);

    // Mailboxes.
    let pump_mbox = Mailbox::new();
    let controller_mbox = Mailbox::new();
    let tank_mbox = Mailbox::new();

    // Connections.
    controller.pump_cmd.connect(Pump::command, &pump_mbox);
    tank.water_sense
        .connect(Controller::water_sense, &controller_mbox);
    pump.flow_rate.connect(Tank::set_flow_rate, &tank_mbox);

    // Sinks.

    // Controller.
    let pump_cmd = EventQueue::new_open();
    controller.pump_cmd.connect_sink(&pump_cmd);

    // Pump.
    let flow_rate = EventQueue::new_open();
    pump.flow_rate.connect_sink(&flow_rate);

    // Tank.
    let water_sense = EventQueue::new_open();
    tank.water_sense.connect_sink(&water_sense);

    // Bench assembly.
    let mut bench = SimInit::new();

    // Controller.
    EventSource::new()
        .connect(Controller::brew_time, &controller_mbox)
        .add_endpoint(&mut bench, "brew_time")
        // FIXME
        .unwrap();

    EventSource::new()
        .connect(Controller::brew_cmd, &controller_mbox)
        .add_endpoint(&mut bench, "brew_cmd")
        // FIXME
        .unwrap();

    // Tank.
    EventSource::new()
        .connect(Tank::fill, &tank_mbox)
        .add_endpoint(&mut bench, "fill")
        // FIXME
        .unwrap();

    QuerySource::new()
        .connect(Tank::volume, &tank_mbox)
        .add_endpoint(&mut bench, "volume")
        // FIXME
        .unwrap();

    bench = bench
        .add_model(controller, controller_mbox, "controller")
        .add_model(pump, pump_mbox, "pump")
        .add_model(tank, tank_mbox, "tank")
        .add_event_sink(pump_cmd.into_reader(), "pump_cmd")?
        .add_event_sink(flow_rate.into_reader(), "flow_rate")?
        .add_event_sink(water_sense.into_reader(), "water_sense")?;

    Ok(bench)
}

/// Run the simulation using the endpoint registry.
///
/// Note that this is just provided for the sake of illustration of the bench
/// API. Most typically, the bench would be exposed by the gRPC server and
/// managed from a remote client.
fn run_simulation(
    mut simu: Simulation,
    mut registry: Endpoints,
    mut t: MonotonicTime,
) -> Result<(), SimulationError> {
    let scheduler = simu.scheduler();

    // Sinks used in simulation.
    let mut flow_rate = registry.take_event_sink::<f64>("flow_rate").unwrap();

    // Sources used in simulation.
    let brew_cmd: EventId<()> = registry.get_event_source_id("brew_cmd").unwrap();
    let brew_time: EventId<Duration> = registry.get_event_source_id("brew_time").unwrap();
    let fill: EventId<f64> = registry.get_event_source_id("fill").unwrap();
    let volume: QueryId<(), f64> = registry.get_query_source_id("volume").unwrap();

    // Check volume.
    let mut volume_reader = simu.process_query(&volume, ())?;
    assert_eq!(volume_reader.read().unwrap().next(), Some(0.0013875));

    // Drink too much coffee.
    let volume_per_shot = PUMP_FLOW_RATE * Controller::DEFAULT_BREW_TIME.as_secs_f64();
    let shots_per_tank = (INIT_TANK_VOLUME / volume_per_shot) as u64; // YOLO--who cares about floating-point rounding errors?
    for _ in 0..(shots_per_tank - 1) {
        simu.process_event(&brew_cmd, ())?;
        assert_eq!(flow_rate.try_read(), Some(PUMP_FLOW_RATE));
        simu.step()?;
        t += Controller::DEFAULT_BREW_TIME;
        assert_eq!(simu.time(), t);
        assert_eq!(flow_rate.try_read(), Some(0.0));
    }

    // Check that the tank becomes empty before the completion of the next shot.
    simu.process_event(&brew_cmd, ())?;
    simu.step()?;
    assert!(simu.time() < t + Controller::DEFAULT_BREW_TIME);
    t = simu.time();
    assert_eq!(flow_rate.try_read(), Some(0.0));
    let mut volume_reader = simu.process_query(&volume, ())?;
    assert_eq!(volume_reader.read().unwrap().next(), Some(0.0));

    // Try to brew another shot while the tank is still empty.
    simu.process_event(&brew_cmd, ())?;
    assert!(flow_rate.try_read().is_none());

    // Change the brew time and fill up the tank.
    let brew_t = Duration::new(30, 0);
    simu.process_event(&brew_time, brew_t)?;
    simu.process_event(&fill, 1.0e-3)?;
    simu.process_event(&brew_cmd, ())?;
    assert_eq!(flow_rate.try_read(), Some(PUMP_FLOW_RATE));

    simu.step()?;
    t += brew_t;
    assert_eq!(simu.time(), t);
    assert_eq!(flow_rate.try_read(), Some(0.0));

    // Interrupt the brew after 15s by pressing again the brew button.
    scheduler
        .schedule_event(Duration::from_secs(15), &brew_cmd, ())
        .unwrap();
    simu.process_event(&brew_cmd, ())?;
    assert_eq!(flow_rate.try_read(), Some(PUMP_FLOW_RATE));

    simu.step()?;
    t += Duration::from_secs(15);
    assert_eq!(simu.time(), t);
    assert_eq!(flow_rate.try_read(), Some(0.0));

    Ok(())
}

fn main() -> Result<(), SimulationError> {
    let bench = build_bench((PUMP_FLOW_RATE, INIT_TANK_VOLUME))?;

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;
    let (mut simu, mut registry) = bench.init_with_registry(t0)?;

    // Sinks used in simulation.
    let mut flow_rate = registry.take_event_sink::<f64>("flow_rate").unwrap();

    // Sources used in simulation.
    let brew_cmd: EventId<()> = registry.get_event_source_id("brew_cmd").unwrap();

    // ----------
    // Simulation.
    // ----------

    // Check initial conditions.
    let mut t = t0;
    assert_eq!(simu.time(), t);

    // Brew one espresso shot with the default brew time.
    simu.process_event(&brew_cmd, ())?;
    assert_eq!(flow_rate.try_read(), Some(PUMP_FLOW_RATE));

    simu.step()?;
    t += Controller::DEFAULT_BREW_TIME;
    assert_eq!(simu.time(), t);
    assert_eq!(flow_rate.try_read(), Some(0.0));

    // Save the current simulation state.
    let saved_time = simu.time();
    let mut state = Vec::new();
    simu.save(&mut state)?;

    // Run the rest of the simulation twice: the second time from the saved
    // state.
    run_simulation(simu, registry, t)?;

    let (simu, registry) = build_bench((PUMP_FLOW_RATE, INIT_TANK_VOLUME))?.restore(&state[..])?;
    run_simulation(simu, registry, saved_time)
}
