use nexosim::model::{BuildContext, Environment, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventBuffer, EventSource, Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};

use serde::{Deserialize, Serialize};

fn bench(_: ()) -> Result<(Simulation, EndpointRegistry), SimulationError> {
    let mut registry = EndpointRegistry::new();

    let model = MyModel;
    let mbox = Mailbox::new();
    let addr = mbox.address();
    let mut env = MyEnv {
        output: Output::default(),
    };

    let mut input = EventSource::new();
    input.connect(MyModel::input, &addr);
    registry.add_event_source(input, "input").unwrap();

    let (sim, _) = SimInit::new()
        .add_model(model, env, mbox, "model")
        .set_clock(AutoSystemClock::new())
        .init(MonotonicTime::EPOCH)
        .unwrap();

    Ok((sim, registry))
}

fn main() {
    server::run(bench, "0.0.0.0:3700".parse().unwrap()).unwrap();
}

#[derive(Serialize, Deserialize)]
struct MyModel;
impl MyModel {
    pub async fn repl(&mut self) -> u16 {
        14
    }
    pub async fn input(&mut self, value: u16, env: &mut MyEnv) {
        println!("@@@@@@@@@@@@@@@ {}", value);
        env.output.send(value);
    }
}
impl Model for MyModel {
    type Environment = MyEnv;
}

struct MyEnv {
    output: Output<u16>,
}
impl Environment for MyEnv {}
