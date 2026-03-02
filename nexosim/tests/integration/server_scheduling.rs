use std::error::Error;
use std::time::Duration;

use prost_types::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot::Sender;
use tokio::task::JoinHandle;

use nexosim::model::Model;
use nexosim::ports::{EventSource, Output, QuerySource, SinkState, event_slot_endpoint};
use nexosim::simulation::{Mailbox, SimInit};

mod grpc_client {
    include!("../codegen/simulation.v1.rs");
}
use grpc_client::{
    BuildRequest, InitRequest, ReadEventRequest, RunRequest, ScheduleEventRequest,
    ScheduleQueryRequest, read_event_reply, schedule_event_request, schedule_query_request,
    simulation_client::SimulationClient,
};

#[derive(Default, Serialize, Deserialize)]
struct SimpleModel {
    output: Output<u32>,
}
#[Model]
impl SimpleModel {
    async fn input(&mut self, value: u32) {
        self.output.send(3 * value).await;
    }
    async fn query(&mut self, value: u32) -> u32 {
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

fn get_free_port() -> u16 {
    let socket = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    socket.local_addr().unwrap().port()
}

async fn get_client<F, I>(
    bench: F,
) -> (
    SimulationClient<tonic::transport::Channel>,
    Sender<()>,
    JoinHandle<()>,
)
where
    F: FnMut(I) -> Result<SimInit, Box<dyn std::error::Error>> + Send + 'static,
    I: serde::de::DeserializeOwned,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    let signal = async move {
        rx.await.unwrap();
    };
    let port = get_free_port();
    let handle = tokio::task::spawn_blocking(move || {
        nexosim::server::run_with_shutdown(
            bench,
            format!("0.0.0.0:{port}").parse().unwrap(),
            signal,
        )
        .unwrap();
    });

    // Make sure the server is up.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut client = SimulationClient::connect(format!("http://0.0.0.0:{port}"))
        .await
        .unwrap();

    let _ = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    let _ = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    (client, tx, handle)
}

#[tokio::test]
async fn event_schedule_simple() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let _ = client
        .schedule_event(ScheduleEventRequest {
            source: Some(grpc_client::Path {
                segments: vec!["input".to_string()],
            }),
            event: vec![7],
            period: None,
            with_key: false,
            deadline: Some(schedule_event_request::Deadline::Duration(
                prost_types::Duration {
                    seconds: 1,
                    ..Default::default()
                },
            )),
        })
        .await
        .unwrap();

    let _ = client.run(RunRequest {}).await.unwrap();

    let response = client
        .read_event(ReadEventRequest {
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

    assert!(
        matches!(response.into_inner().result.unwrap(), read_event_reply::Result::Event(a) if a  == vec![21])
    );

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn query_schedule_simple() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let mut query_client = client.clone();
    let response_later = tokio::spawn(async move {
        query_client
            .schedule_query(ScheduleQueryRequest {
                source: Some(grpc_client::Path {
                    segments: vec!["query".to_string()],
                }),
                request: vec![5],
                deadline: Some(schedule_query_request::Deadline::Duration(
                    prost_types::Duration {
                        seconds: 3,
                        ..Default::default()
                    },
                )),
            })
            .await
    });

    // Wait a bit so the 'later' query gets already scheduled scheduled and has a
    // replier pending.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut query_client = client.clone();
    let response_earlier = tokio::spawn(async move {
        query_client
            .schedule_query(ScheduleQueryRequest {
                source: Some(grpc_client::Path {
                    segments: vec!["query".to_string()],
                }),
                request: vec![11],
                deadline: Some(schedule_query_request::Deadline::Duration(
                    prost_types::Duration {
                        seconds: 1,
                        ..Default::default()
                    },
                )),
            })
            .await
    });

    // This is necessary to make 'sure' that the queries get scheduled before the
    // simulation is started. (otherwise they won't be executed!)
    tokio::time::sleep(Duration::from_millis(500)).await;

    let _ = client.run(RunRequest {}).await.unwrap();

    let response = response_earlier.await.unwrap().unwrap();
    let reply: u32 = ciborium::from_reader(&response.into_inner().replies[0][..]).unwrap();
    assert_eq!(reply, 77);

    let response = response_later.await.unwrap().unwrap();
    let reply: u32 = ciborium::from_reader(&response.into_inner().replies[0][..]).unwrap();
    assert_eq!(reply, 35);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}
