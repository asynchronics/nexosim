use nexosim::model::{BuildContext, Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventBuffer, EventSource, Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::{MonotonicTime, SystemClock};

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

fn bench(cfg: usize) -> Result<(SimInit, EndpointRegistry), SimulationError> {
    let mut registry = EndpointRegistry::new();

    // let mut source = EventSource::new();
    // source.connect(MyModel::input, &mbox);

    let mut sim_init = SimInit::new()
        .set_clock(SystemClock::from_instant(
            MonotonicTime::EPOCH,
            Instant::now(),
        ))
        .set_clock_tolerance(Duration::from_secs(2));

    for i in 0..cfg {
        let model = MyModel {
            output: Output::default(),
        };
        let mbox = Mailbox::new();
        let addr = mbox.address();

        let mut input = EventSource::new();
        input.connect(MyModel::input, &addr);
        registry
            .add_event_source(input, format!("input_{}", i))
            .unwrap();

        sim_init = sim_init.add_model(model, mbox, "model");
    }

    // let input_id = sim_init.register_event_source(source);

    sim_init = sim_init.with_post_restore(|sim, _| {
        sim.reset_clock(SystemClock::from_instant(sim.time(), Instant::now()))
    });

    Ok((sim_init, registry))
}

fn main() {
    server::run(bench, "0.0.0.0:3700".parse().unwrap()).unwrap();
}

#[derive(Serialize, Deserialize)]
struct MyModel {
    output: Output<u16>,
}
impl MyModel {
    pub async fn input(&mut self, value: u16, cx: &mut Context<Self>) {
        println!("{} {}", value, cx.time());
        self.output.send(value);
    }
}
impl Model for MyModel {
    type Environment = ();
}
