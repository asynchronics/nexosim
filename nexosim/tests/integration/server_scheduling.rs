use std::error::Error;
use std::time::Duration;

use prost_types::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot::Sender;
use tokio::task::JoinHandle;

use nexosim::model::{Context, Model};
use nexosim::ports::{EventSource, Output, QuerySource, SinkState, event_slot_endpoint};
use nexosim::simulation::{Mailbox, SimInit};

use super::server_utils::get_client;
use super::server_utils::grpc_client::{
    BuildRequest, InitRequest, Path, ReadEventRequest, ScheduleEventRequest, ScheduleQueryRequest,
    StepUntilRequest, read_event_reply, schedule_event_request, schedule_query_request,
    simulation_client::SimulationClient, step_until_request,
};
use crate::some_deadline_secs;

#[derive(Default, Serialize, Deserialize)]
struct SimpleModel {
    output: Output<u32>,
}
#[Model]
impl SimpleModel {
    async fn input(&mut self, value: u32) {
        self.output.send(3 * value).await;
    }
    async fn query(&mut self, value: i64, cx: &Context<Self>) -> i64 {
        7 * value * cx.time().as_secs()
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

async fn init_client<F, I>(
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
    let (mut client, signal, handle) = get_client(bench).await;

    let _ = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    let _ = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    (client, signal, handle)
}

#[tokio::test]
async fn event_schedule_simple() {
    let (mut client, signal, handle) = init_client(simple_bench).await;

    let _ = client
        .schedule_event(ScheduleEventRequest {
            source: Some(Path {
                segments: vec!["input".to_string()],
            }),
            event: vec![7],
            period: None,
            with_key: false,
            deadline: some_deadline_secs!(1, schedule_event_request),
        })
        .await
        .unwrap();

    let _ = client
        .schedule_event(ScheduleEventRequest {
            source: Some(Path {
                segments: vec!["input".to_string()],
            }),
            event: vec![7],
            period: None,
            with_key: false,
            deadline: some_deadline_secs!(3, schedule_event_request),
        })
        .await
        .unwrap();

    // Check output between scheduled events.
    let _ = client
        .step_until(StepUntilRequest {
            deadline: some_deadline_secs!(2, step_until_request),
        })
        .await
        .unwrap();

    let response = client
        .read_event(ReadEventRequest {
            sink: Some(Path {
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

    // Check the final output.
    let _ = client
        .step_until(StepUntilRequest {
            deadline: some_deadline_secs!(3, step_until_request),
        })
        .await
        .unwrap();

    let response = client
        .read_event(ReadEventRequest {
            sink: Some(Path {
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
    let (mut client, signal, handle) = init_client(simple_bench).await;

    let mut query_client = client.clone();
    let response_later = tokio::spawn(async move {
        query_client
            .schedule_query(ScheduleQueryRequest {
                source: Some(Path {
                    segments: vec!["query".to_string()],
                }),
                request: vec![5],
                deadline: some_deadline_secs!(3, schedule_query_request),
            })
            .await
    });

    let mut query_client = client.clone();
    let response_earlier = tokio::spawn(async move {
        query_client
            .schedule_query(ScheduleQueryRequest {
                source: Some(Path {
                    segments: vec!["query".to_string()],
                }),
                request: vec![11],
                deadline: some_deadline_secs!(1, schedule_query_request),
            })
            .await
    });

    // Make sure that the simulation won't run before the query threads
    // execute scheduling.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let _ = client
        .step_until(StepUntilRequest {
            deadline: some_deadline_secs!(4, step_until_request),
        })
        .await
        .unwrap();

    let response = response_earlier.await.unwrap().unwrap();
    let reply: u32 = ciborium::from_reader(&response.into_inner().replies[0][..]).unwrap();
    assert_eq!(reply, 7 * 11);

    let response = response_later.await.unwrap().unwrap();
    let reply: u32 = ciborium::from_reader(&response.into_inner().replies[0][..]).unwrap();
    assert_eq!(reply, 7 * 5 * 3);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}
