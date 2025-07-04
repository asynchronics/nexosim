use nexosim::model::Model;
use nexosim::ports::QuerySource;
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::MonotonicTime;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct ModelConfig {
    foo: u16,
    bar: String,
}

#[derive(Serialize, Deserialize)]
struct MyReply {
    value: u32,
}

#[derive(Default)]
pub(crate) struct MyModel {
    foo: u16,
    bar: String,
}

impl MyModel {
    pub async fn my_replier(&mut self) -> MyReply {
        MyReply { value: 0 }
    }

    pub(crate) fn new(cfg: ModelConfig) -> Self {
        let ModelConfig { foo, bar } = cfg;
        Self { foo, bar }
    }
}

impl Model for MyModel {}

fn bench(cfg: ModelConfig) -> Result<(Simulation, EndpointRegistry), SimulationError> {
    let model = MyModel::new(cfg);

    // Mailboxes.
    let model_mbox = Mailbox::new();
    let model_addr = model_mbox.address();

    // Endpoints.
    let mut registry = EndpointRegistry::new();

    let mut input = QuerySource::new();
    input.connect(MyModel::my_replier, &model_addr);
    registry.add_query_source(input, "replier").unwrap();

    // Assembly and initialization.
    let sim = SimInit::new()
        .add_model(model, model_mbox, "model")
        .init(MonotonicTime::EPOCH)?
        .0;

    Ok((sim, registry))
}

fn main() {
    server::run(bench, "0.0.0.0:41633".parse().unwrap()).unwrap();
}
