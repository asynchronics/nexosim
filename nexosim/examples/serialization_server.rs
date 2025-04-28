use nexosim::model::{BuildContext, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventBuffer, EventSource, Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};

use serde::{Deserialize, Serialize};

fn bench(id: String) -> Result<(Simulation, EndpointRegistry), SimulationError> {
    let mut registry = EndpointRegistry::new();

    let model = MyModel {
        id,
        output: Output::default(),
    };
    let mbox = Mailbox::new();
    let addr = mbox.address();

    let mut input = EventSource::new();
    input.connect(MyModel::input, &addr);
    registry.add_event_source(input, "input").unwrap();

    let (sim, _) = SimInit::new()
        .add_model(model, mbox, "model")
        .set_clock(AutoSystemClock::new())
        .init(MonotonicTime::EPOCH)
        .unwrap();

    Ok((sim, registry))
}

fn main() {
    server::run(bench, "0.0.0.0:3700".parse().unwrap()).unwrap();
}

#[derive(Serialize, Deserialize)]
struct MyModel {
    id: String,
    output: Output<String>,
}
impl MyModel {
    pub async fn repl(&mut self) -> u16 {
        14
    }
    pub async fn input(&mut self, value: String) {
        println!("{} {}", self.id, value);
        self.output.send(value);
    }
}
impl Model for MyModel {
    type Environment = ();
}
