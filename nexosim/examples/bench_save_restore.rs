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
use nexosim::ports::{
    EventSinkReader, EventSource, QuerySource, SinkState, event_queue_endpoint, event_slot_endpoint,
};
use nexosim::simulation::{
    BenchError, EventId, Mailbox, QueryId, SimInit, Simulation, SimulationError,
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
/// If the `BenchError` was mapped to `Box<dyn Error>`, the same function could
/// be used to build a gRPC server and manage the simulation from a gRPC Python
/// client (see the `server` feature).
pub fn build_bench((pump_flow_rate, init_tank_volume): (f64, f64)) -> Result<SimInit, BenchError> {
    // Models.
    let mut pump = Pump::new(pump_flow_rate);
    let mut controller = Controller::new();
    let mut tank = Tank::new(init_tank_volume);

    let pump_mbox = Mailbox::new();
    let controller_mbox = Mailbox::new();
    let tank_mbox = Mailbox::new();

    // Connections.
    controller.pump_cmd.connect(Pump::command, &pump_mbox);
    tank.water_sense
        .connect(Controller::water_sense, &controller_mbox);
    pump.flow_rate.connect(Tank::set_flow_rate, &tank_mbox);

    // Endpoints.
    let mut bench = SimInit::new();

    EventSource::new()
        .connect(Controller::brew_time, &controller_mbox)
        .bind_endpoint(&mut bench, "brew_time")?;
    EventSource::new()
        .connect(Controller::brew_cmd, &controller_mbox)
        .bind_endpoint(&mut bench, "brew_cmd")?;
    EventSource::new()
        .connect(Tank::fill, &tank_mbox)
        .bind_endpoint(&mut bench, "fill")?;
    QuerySource::new()
        .connect(Tank::volume, &tank_mbox)
        .bind_endpoint(&mut bench, "volume")?;

    let sink = event_queue_endpoint(&mut bench, SinkState::Enabled, "pump_cmd")?;
    controller.pump_cmd.connect_sink(sink);
    let sink = event_slot_endpoint(&mut bench, SinkState::Enabled, "flow_rate")?;
    pump.flow_rate.connect_sink(sink);
    let sink = event_slot_endpoint(&mut bench, SinkState::Enabled, "water_sense")?;
    tank.water_sense.connect_sink(sink);

    // Bench assembly.
    bench = bench
        .add_model(controller, controller_mbox, "controller")
        .add_model(pump, pump_mbox, "pump")
        .add_model(tank, tank_mbox, "tank");

    Ok(bench)
}

/// Run the simulation using the endpoint registry.
///
/// Note that this is just provided for the sake of illustration of the bench
/// API. Most typically, the bench would be exposed by the gRPC server and
/// managed from a remote client.
fn run_simulation(
    mut simu: Simulation,
    registry: Endpoints,
    mut t: MonotonicTime,
    flow_rate: &mut Box<dyn EventSinkReader<f64>>,
) -> Result<(), SimulationError> {
    let scheduler = simu.scheduler();

    // Sources used in simulation.
    let brew_cmd: EventId<()> = registry.get_event_source_id("brew_cmd").unwrap();
    let brew_time: EventId<Duration> = registry.get_event_source_id("brew_time").unwrap();
    let fill: EventId<f64> = registry.get_event_source_id("fill").unwrap();
    let volume: QueryId<(), f64> = registry.get_query_source_id("volume").unwrap();

    // Check volume.
    assert_eq!(simu.process_query(&volume, ()).unwrap(), 0.0013875);

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
    assert_eq!(simu.process_query(&volume, ()).unwrap(), 0.0);

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
    let mut bench = build_bench((PUMP_FLOW_RATE, INIT_TANK_VOLUME))?;

    let t0 = MonotonicTime::EPOCH; // arbitrary since models do not depend on absolute time
    let mut endpoints = bench.take_endpoints();
    let mut simu = bench.init(t0)?;

    // Sinks used in simulation.
    let mut flow_rate = endpoints.take_event_sink::<f64>("flow_rate").unwrap();

    // Sources used in simulation.
    let brew_cmd: EventId<()> = endpoints.get_event_source_id("brew_cmd").unwrap();

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
    run_simulation(simu, endpoints, t, &mut flow_rate)?;

    let mut bench = build_bench((PUMP_FLOW_RATE, INIT_TANK_VOLUME))?;
    let mut endpoints = bench.take_endpoints();
    let simu = bench.restore(&state[..])?;

    // Extract the sink again as the previous bench instance is dropped.
    let mut flow_rate = endpoints.take_event_sink::<f64>("flow_rate").unwrap();

    run_simulation(simu, endpoints, saved_time, &mut flow_rate)
}
