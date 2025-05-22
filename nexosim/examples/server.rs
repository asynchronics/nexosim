use nexosim::model::{BuildContext, Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventSource, Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::{MonotonicTime, SystemClock};

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

fn bench(cfg: usize) -> (SimInit, EndpointRegistry) {
    let mut registry = EndpointRegistry::new();

    let mbox = Mailbox::new();
    let addr = mbox.address();
    let model = MyModel {
        state: 0,
        output: Output::default(),
    };

    let mut input = EventSource::new();
    input.connect(MyModel::input, &addr);

    let mut query = QuerySource::new();
    query.connect(MyModel::query, &addr);

    let sim_init = SimInit::new()
        .set_clock(SystemClock::from_instant(
            MonotonicTime::EPOCH,
            Instant::now(),
        ))
        .add_model(model, mbox, "model");

    registry.add_event_source(input, "input").unwrap();
    registry.add_query_source(query, "query").unwrap();

    (sim_init, registry)
}

fn main() {
    server::run(bench, "0.0.0.0:3700".parse().unwrap()).unwrap();
}

#[derive(Serialize, Deserialize)]
struct MyModel {
    state: u16,
    output: Output<u16>,
}
impl MyModel {
    pub async fn input(&mut self, value: u16, cx: &mut Context<Self>) {
        self.state = value;
        println!("{} {}", value, cx.time());
        self.output.send(value).await;
    }
    pub async fn query(&mut self) -> u16 {
        self.state
    }
}
impl Model for MyModel {
    type Env = ();
}
