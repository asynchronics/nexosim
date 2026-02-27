use std::error::Error;
use std::time::Duration;

use prost_types::Timestamp;
use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::ports::{EventSource, Output, QuerySource, SinkState, event_slot_endpoint};
use nexosim::simulation::{Mailbox, SimInit};

mod grpc_client {
    include!("../codegen/simulation.v1.rs");
}

#[derive(Default, Serialize, Deserialize)]
struct SimpleModel {
    output: Output<u8>,
}
#[Model]
impl SimpleModel {
    async fn input(&mut self, value: u8) {
        self.output.send(value).await;
    }
    async fn query(&mut self, value: u8) -> u8 {
        7 * value
    }
}

pub fn simple_bench(_: u8) -> Result<SimInit, Box<dyn Error>> {
    let mut simple = SimpleModel::default();

    let simple_mbox = Mailbox::new();

    let mut bench = SimInit::new();

    EventSource::new()
        .connect(SimpleModel::input, &simple_mbox)
        .bind_endpoint(&mut bench, "input")?;
    QuerySource::new()
        .connect(SimpleModel::query, &simple_mbox)
        .bind_endpoint(&mut bench, "query")?;

    let sink = event_slot_endpoint(&mut bench, SinkState::Enabled, "sink")?;
    simple.output.connect_sink(sink);

    // Bench assembly.
    bench = bench.add_model(simple, simple_mbox, "simple");
    Ok(bench)
}

#[tokio::test]
async fn event_schedule_simple() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let signal = async move {
        rx.await.unwrap();
    };
    let handle = tokio::task::spawn_blocking(|| {
        nexosim::server::run_with_shutdown(simple_bench, "0.0.0.0:55576".parse().unwrap(), signal)
            .unwrap();
    });

    // Make sure the server is up.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut client =
        grpc_client::simulation_client::SimulationClient::connect("http://0.0.0.0:55576")
            .await
            .unwrap();

    let response = client
        .build(grpc_client::BuildRequest { cfg: vec![0] })
        .await
        .unwrap();

    println!("@@@@@ {response:?}");

    let response = client
        .schedule_event(grpc_client::ScheduleEventRequest {
            source: Some(grpc_client::Path {
                segments: vec!["input".to_string()],
            }),
            event: vec![7],
            period: None,
            with_key: false,
            deadline: Some(grpc_client::schedule_event_request::Deadline::Duration(
                prost_types::Duration {
                    seconds: 1,
                    ..Default::default()
                },
            )),
        })
        .await
        .unwrap();

    println!("@@@@@ {response:?}");

    let response = client
        .init_and_run(grpc_client::InitAndRunRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();

    println!("@@@@@ {response:?}");

    tokio::time::sleep(Duration::from_secs(1)).await;
    let response = client
        .read_event(grpc_client::ReadEventRequest {
            sink: Some(grpc_client::Path {
                segments: vec!["sink".to_string()],
            }),
            timeout: Some(prost_types::Duration {
                seconds: 1,
                ..Default::default()
            }),
        })
        .await
        .unwrap();

    println!("@@@@@ {response:?}");

    // Shutdown.
    tx.send(()).unwrap();
    handle.await.unwrap();
    assert!(false);
}
