use nexosim::model::{BuildContext, Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventSource, Output, QuerySource};
use nexosim::registry::EndpointRegistry;
use nexosim::simulation::{Mailbox, SimInit, Simulation, SimulationError, SourceId};
use nexosim::time::{MonotonicTime, SystemClock};

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

fn bench(_: usize) -> (SimInit, EndpointRegistry) {
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
    let (mut simu, registry) =
        nexosim::server::init_bench(bench, 12, MonotonicTime::EPOCH).unwrap();
    let source_id: SourceId<u16> = registry.get_source_id("input").unwrap();

    let scheduler = simu.scheduler();
    scheduler
        .schedule_periodic_event(
            Duration::from_secs(1),
            Duration::from_secs(2),
            &source_id,
            17,
        )
        .unwrap();

    simu.step().unwrap();

    let mut state = Vec::new();
    simu.save_with_cfg(12, &mut state).unwrap();

    simu.step().unwrap();

    let (mut simu, _) = nexosim::server::restore_bench(bench, &state, None).unwrap();

    simu.step().unwrap();
    simu.step().unwrap();
    simu.step().unwrap();
    // server::run(bench, "0.0.0.0:3700".parse().unwrap()).unwrap();
}

#[derive(Serialize, Deserialize)]
struct MyModel {
    state: u16,
    output: Output<u16>,
}
impl MyModel {
    pub async fn input(&mut self, value: u16, cx: &mut Context<Self>) {
        self.state += value;
        println!("{} {}", self.state, cx.time());
        self.output.send(value).await;
    }
    pub async fn query(&mut self) -> u16 {
        self.state
    }
}
impl Model for MyModel {
    type Env = ();
}
