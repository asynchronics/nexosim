use nexosim::model::{BuildContext, Environment, InitializedModel, Model, ProtoModel};
use nexosim::ports::{Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::MonotonicTime;

use serde::{Deserialize, Serialize};

fn bench(cfg: u16) -> Result<(Simulation, EndpointRegistry), SimulationError> {
    let mut registry = EndpointRegistry::new();

    let model = MyModel;
    let mbox = Mailbox::new();
    let addr = mbox.address();
    let env = MyEnv;

    let mut replier = QuerySource::new();
    replier.connect(MyModel::repl, &addr);
    registry.add_query_source(replier, "replier").unwrap();

    let (sim, _) = SimInit::new()
        .add_model(model, env, mbox, "model")
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
}
impl Model for MyModel {
    type Environment = MyEnv;
}

struct MyEnv;
impl Environment for MyEnv {}
