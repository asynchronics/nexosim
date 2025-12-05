//! Example: espresso coffee machine.
//!
//! This example demonstrates in particular:
//!
//! * non-trivial state machines,
//! * cancellation of events,
//! * model initialization,
//! * simulation monitoring,
//! * bench construction,
//! * end points registry.
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

use nexosim::ports::{EventQueue, EventSlot, EventSource, QuerySource};
use nexosim::simulation::{Mailbox, SimInit, SimulationError, SourceId};
use nexosim::time::MonotonicTime;

mod espresso_machine;

pub use espresso_machine::{Controller, Pump, Tank};

pub fn bench((pump_flow_rate, init_tank_volume): (f64, f64)) -> SimInit {
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
    let pump_cmd = EventQueue::new();
    controller.pump_cmd.connect_sink(&pump_cmd);

    // Pump.
    let flow_rate = EventSlot::new();
    pump.flow_rate.connect_sink(&flow_rate);

    // Tank.
    let water_sense = EventSlot::new();
    tank.water_sense.connect_sink(&water_sense);

    // Sources.

    // Controller.
    let mut brew_time = EventSource::new();
    brew_time.connect(Controller::brew_time, &controller_mbox);

    let mut brew_cmd = EventSource::new();
    brew_cmd.connect(Controller::brew_cmd, &controller_mbox);

    // Tank.
    let mut fill = EventSource::new();
    fill.connect(Tank::fill, &tank_mbox);

    let mut volume = QuerySource::new();
    volume.connect(Tank::volume, &tank_mbox);

    // Bench assembly.
    let mut bench = SimInit::new()
        .add_model(controller, controller_mbox, "controller")
        .add_model(pump, pump_mbox, "pump")
        .add_model(tank, tank_mbox, "tank");

    bench
        .add_event_sink_endpoint(pump_cmd.into_reader(), "pump_cmd")
        .unwrap();
    bench
        .add_event_sink_endpoint(flow_rate, "flow_rate")
        .unwrap();
    bench
        .add_event_sink_endpoint(water_sense, "water_sense")
        .unwrap();

    bench
        .add_event_source_endpoint(brew_time, "brew_time")
        .unwrap();
    bench
        .add_event_source_endpoint(brew_cmd, "brew_cmd")
        .unwrap();
    bench.add_event_source_endpoint(fill, "fill").unwrap();

    bench.add_query_source_endpoint(volume, "volume").unwrap();

    bench
}

fn main() -> Result<(), SimulationError> {
    // The constant mass flow rate assumption is of course a gross
    // simplification, so the flow rate is set to an expected average over the
    // whole extraction [m³·s⁻¹].
    let pump_flow_rate = 4.5e-6;
    // Start with 1.5l in the tank [m³].
    let init_tank_volume = 1.5e-3;

    let bench = bench((pump_flow_rate, init_tank_volume));

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;
    let (mut simu, registry) = bench.init(t0)?;
    let scheduler = simu.scheduler();

    // Sinks used in simulation.
    let mut flow_rate: EventSlot<f64> = registry.get_sink("flow_rate").unwrap();

    // Sources used in simulation.
    let brew_cmd: &EventSource<()> = registry.get_event_source("brew_cmd").unwrap();
    let brew_time: &EventSource<Duration> = registry.get_event_source("brew_time").unwrap();
    let fill: &EventSource<f64> = registry.get_event_source("fill").unwrap();
    let volume: &QuerySource<(), f64> = registry.get_query_source("volume").unwrap();

    // Source IDs for scheduling.
    let brew_source_id: SourceId<()> = registry.get_event_source_id("brew_cmd").unwrap();

    // ----------
    // Simulation.
    // ----------

    // Check initial conditions.
    let mut t = t0;
    assert_eq!(simu.time(), t);

    // Brew one espresso shot with the default brew time.
    simu.process_action(brew_cmd.action(()))?;
    assert_eq!(flow_rate.next(), Some(pump_flow_rate));

    simu.step()?;
    t += Controller::DEFAULT_BREW_TIME;
    assert_eq!(simu.time(), t);
    assert_eq!(flow_rate.next(), Some(0.0));

    let (volume_action, mut volume_reply) = volume.query(());
    simu.process_action(volume_action)?;
    assert_eq!(volume_reply.take().unwrap().next(), Some(0.0013875));

    // Drink too much coffee.
    let volume_per_shot = pump_flow_rate * Controller::DEFAULT_BREW_TIME.as_secs_f64();
    let shots_per_tank = (init_tank_volume / volume_per_shot) as u64; // YOLO--who cares about floating-point rounding errors?
    for _ in 0..(shots_per_tank - 1) {
        simu.process_action(brew_cmd.action(()))?;
        assert_eq!(flow_rate.next(), Some(pump_flow_rate));
        simu.step()?;
        t += Controller::DEFAULT_BREW_TIME;
        assert_eq!(simu.time(), t);
        assert_eq!(flow_rate.next(), Some(0.0));
    }

    // Check that the tank becomes empty before the completion of the next shot.
    simu.process_action(brew_cmd.action(()))?;
    simu.step()?;
    assert!(simu.time() < t + Controller::DEFAULT_BREW_TIME);
    t = simu.time();
    assert_eq!(flow_rate.next(), Some(0.0));
    let (volume_action, mut volume_reply) = volume.query(());
    simu.process_action(volume_action)?;
    assert_eq!(volume_reply.take().unwrap().next(), Some(0.0));

    // Try to brew another shot while the tank is still empty.
    simu.process_action(brew_cmd.action(()))?;
    assert!(flow_rate.next().is_none());

    // Change the brew time and fill up the tank.
    let brew_t = Duration::new(30, 0);
    simu.process_action(brew_time.action(brew_t))?;
    simu.process_action(fill.action(1.0e-3))?;
    simu.process_action(brew_cmd.action(()))?;
    assert_eq!(flow_rate.next(), Some(pump_flow_rate));

    simu.step()?;
    t += brew_t;
    assert_eq!(simu.time(), t);
    assert_eq!(flow_rate.next(), Some(0.0));

    // Interrupt the brew after 15s by pressing again the brew button.
    scheduler
        .schedule_event(Duration::from_secs(15), &brew_source_id, ())
        .unwrap();
    simu.process_action(brew_cmd.action(()))?;
    assert_eq!(flow_rate.next(), Some(pump_flow_rate));

    simu.step()?;
    t += Duration::from_secs(15);
    assert_eq!(simu.time(), t);
    assert_eq!(flow_rate.next(), Some(0.0));

    Ok(())
}
