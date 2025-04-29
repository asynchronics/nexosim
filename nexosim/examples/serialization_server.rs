use nexosim::model::{BuildContext, Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventBuffer, EventSource, Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};

use std::time::Duration;

use serde::{Deserialize, Serialize};

fn bench(_: ()) -> Result<(SimInit, EndpointRegistry), SimulationError> {
    let mut registry = EndpointRegistry::new();

    let model = MyModel {
        output: Output::default(),
    };
    let mbox = Mailbox::new();
    let addr = mbox.address();

    let mut input = EventSource::new();
    input.connect(MyModel::input, &addr);
    registry.add_event_source(input, "input").unwrap();

    let mut source = EventSource::new();
    source.connect(MyModel::input, &mbox);

    let mut sim_init = SimInit::new()
        .add_model(model, mbox, "model")
        .set_clock(AutoSystemClock::new());

    let input_id = sim_init.register_event_source(source);

    sim_init = sim_init
        .with_post_init(move |_, scheduler| {
            println!("Init");
            scheduler.schedule_event(Duration::from_secs(5), input_id, 19);
        })
        .with_post_restore(|_, _| println!("Restored!"));

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
    pub async fn input(&mut self, value: u16) {
        println!("@@@@@@@@@@@@@@@ {}", value);
        self.output.send(value);
    }
}
impl Model for MyModel {
    type Environment = ();
}
