use nexosim::model::Model;
use nexosim::ports::{EventBuffer, EventQueue, EventSource, Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::server;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError};
use nexosim::time::{MonotonicTime, SystemClock};

use std::time::Instant;

use serde::Deserialize;

#[derive(Clone, Deserialize, schemars::JsonSchema)]
enum SumInput {
    None,
    Zero,
    Value(u16),
    ValueStr(String),
}

struct MyModel {
    state: u16,
    output: Output<u16>,
}
impl MyModel {
    pub async fn input(&mut self, value: u16) {
        self.state += value;
        self.output.send(self.state).await;
    }
    pub async fn string_input(&mut self, value: String) {
        self.state += value.parse::<u16>().unwrap();
        self.output.send(self.state).await;
    }
    pub async fn enum_input(&mut self, value: SumInput) {
        // self.state += value.parse::<u16>().unwrap();
        // self.output.send(self.state).await;
    }
    pub async fn query(&mut self, arg: u16) -> u16 {
        arg * self.state
    }
}
impl Model for MyModel {}

fn bench(_: ()) -> Result<(Simulation, EndpointRegistry), SimulationError> {
    let mut registry = EndpointRegistry::new();

    let mbox = Mailbox::new();
    let addr = mbox.address();
    let mut model = MyModel {
        state: 0,
        output: Output::default(),
    };

    let mut input = EventSource::new();
    input.connect(MyModel::input, &addr);
    let mut string_input = EventSource::new();
    string_input.connect(MyModel::string_input, &addr);
    let mut enum_input = EventSource::new();
    enum_input.connect(MyModel::enum_input, &addr);

    let mut query = QuerySource::new();
    query.connect(MyModel::query, &addr);

    let output = EventQueue::new();
    model.output.connect_sink(&output);

    registry.add_event_source(input, "input").unwrap();
    registry
        .add_event_source(string_input, "string_input")
        .unwrap();
    registry.add_event_source(enum_input, "enum_input").unwrap();
    registry.add_query_source(query, "query").unwrap();
    registry
        .add_event_sink(output.into_reader(), "output")
        .unwrap();

    let simu = SimInit::new()
        .set_clock(SystemClock::from_instant(
            MonotonicTime::EPOCH,
            Instant::now(),
        ))
        .add_model(model, mbox, "model")
        .init(MonotonicTime::EPOCH)?
        .0;

    Ok((simu, registry))
}

fn main() {
    server::run(bench, "0.0.0.0:3700".parse().unwrap()).unwrap();
}
