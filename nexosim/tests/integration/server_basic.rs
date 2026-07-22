use prost_types::Timestamp;
use serde::{Deserialize, Serialize};

use nexosim::model::{Context, Model};
use nexosim::ports::{EventSource, QuerySource};
use nexosim::simulation::{Mailbox, SimInit};

use super::server_utils::get_client;
use super::server_utils::grpc_client::{
    BuildReply, BuildRequest, Error, ErrorCode, InitReply, InitRequest, Path, ProcessEventReply,
    ProcessEventRequest, ProcessQueryReply, ProcessQueryRequest, RestoreReply, RestoreRequest,
    RunReply, RunRequest, SaveReply, SaveRequest, ScheduleEventReply, ScheduleEventRequest,
    StepUntilRequest, TerminateReply, TerminateRequest, build_reply, init_reply,
    process_event_reply, process_query_reply, restore_reply, run_reply, save_reply,
    schedule_event_reply, schedule_event_request, simulation_client::SimulationClient,
    step_until_request, terminate_reply,
};
use crate::some_deadline_secs;

macro_rules! assert_resp_ok {
    ($resp:ident, $type:ident, $module:ident) => {
        match $resp.into_inner() {
            $type {
                result: Some($module::Result::Empty(())),
            } => (),
            a => panic!("Expected Ok, got: {:?}", a),
        };
    };
}
macro_rules! assert_resp_err {
    ($resp:ident, $type:ident, $module:ident, $code:expr) => {
        match $resp.into_inner() {
            $type {
                result: Some($module::Result::Error(Error { code, .. })),
            } if code == $code as i32 => (),
            a => panic!("Expected Err: {:?}, got: {:?}", $code, a),
        };
    };
}

#[derive(Default, Serialize, Deserialize)]
struct SimpleModel(i64);
#[Model]
impl SimpleModel {
    async fn input(&mut self, value: i64) {
        self.0 = value;
    }
    async fn state_query(&mut self) -> i64 {
        self.0
    }
    async fn time_query(&mut self, value: i64, cx: &Context<Self>) -> i64 {
        value * cx.time().as_secs()
    }
}

fn simple_bench(_: u8) -> Result<SimInit, Box<dyn std::error::Error>> {
    let simple = SimpleModel::default();
    let simple_mbox = Mailbox::new();

    let mut bench = SimInit::new();

    EventSource::new()
        .connect(SimpleModel::input, &simple_mbox)
        .bind_endpoint(&mut bench, "input")?;

    QuerySource::new()
        .connect(SimpleModel::state_query, &simple_mbox)
        .bind_endpoint(&mut bench, "state_query")?;

    QuerySource::new()
        .connect(SimpleModel::time_query, &simple_mbox)
        .bind_endpoint(&mut bench, "time_query")?;

    // Bench assembly.
    bench = bench.add_model(simple, simple_mbox, "simple");
    Ok(bench)
}

#[tokio::test]
async fn init_before_build_fail() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();

    assert_resp_err!(resp, InitReply, init_reply, ErrorCode::BenchNotBuilt);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn restore_before_build_fail() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    // WARN This state `should` be valid. Verify on test bench change.
    // Invalid state should not ifluence the test though.
    let state = vec![1, 1, 14, 1, 0, 0, 0];
    let resp = client.restore(RestoreRequest { state }).await.unwrap();

    assert_resp_err!(resp, RestoreReply, restore_reply, ErrorCode::BenchNotBuilt);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn subsequent_build_fail() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_err!(resp, BuildReply, build_reply, ErrorCode::BenchAlreadyBuilt);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn build_terminate_build() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    let resp = client.terminate(TerminateRequest {}).await.unwrap();
    assert_resp_ok!(resp, TerminateReply, terminate_reply);

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn build_after_init_fail() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    assert_resp_ok!(resp, InitReply, init_reply);

    let _ = client
        .step_until(StepUntilRequest {
            deadline: some_deadline_secs!(3, step_until_request),
        })
        .await
        .unwrap();

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_err!(resp, BuildReply, build_reply, ErrorCode::BenchAlreadyBuilt);

    // Verify that the simulation is sill alive and at a right ts.
    let resp = client
        .process_query(ProcessQueryRequest {
            source: Some(Path {
                segments: vec!["time_query".to_string()],
            }),
            request: vec![3],
        })
        .await
        .unwrap();
    match resp.into_inner() {
        ProcessQueryReply {
            result: Some(process_query_reply::Result::Empty(())),
            replies,
        } if replies == vec![vec![9]] => (),
        a => panic!("Expected replies: [[9]], got: {:?}", a),
    };

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn no_build_init_build() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    assert_resp_err!(resp, InitReply, init_reply, ErrorCode::BenchNotBuilt);

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn subsequent_init_fail() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    // First init.
    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    assert_resp_ok!(resp, InitReply, init_reply);

    // Step the simulation.
    let _ = client
        .step_until(StepUntilRequest {
            deadline: some_deadline_secs!(2, step_until_request),
        })
        .await
        .unwrap();

    // Second init attempt.
    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    assert_resp_err!(resp, InitReply, init_reply, ErrorCode::BenchNotBuilt);

    // Verify that the simulation is sill alive and at a right ts.
    let resp = client
        .process_query(ProcessQueryRequest {
            source: Some(Path {
                segments: vec!["time_query".to_string()],
            }),
            request: vec![3],
        })
        .await
        .unwrap();
    match resp.into_inner() {
        ProcessQueryReply {
            result: Some(process_query_reply::Result::Empty(())),
            replies,
        } if replies == vec![vec![6]] => (),
        a => panic!("Expected replies: [[6]], got: {:?}", a),
    };

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn init_no_build_restore_fail() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    // First init.
    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    assert_resp_ok!(resp, InitReply, init_reply);

    // Step the simulation.
    let _ = client
        .step_until(StepUntilRequest {
            deadline: some_deadline_secs!(2, step_until_request),
        })
        .await
        .unwrap();

    let state = fetch_state(&mut client).await;

    // Now try restore.
    let resp = client.restore(RestoreRequest { state }).await.unwrap();
    assert_resp_err!(resp, RestoreReply, restore_reply, ErrorCode::BenchNotBuilt);

    // Verify that the simulation is sill alive and at a right ts.
    let resp = client
        .process_query(ProcessQueryRequest {
            source: Some(Path {
                segments: vec!["time_query".to_string()],
            }),
            request: vec![3],
        })
        .await
        .unwrap();
    match resp.into_inner() {
        ProcessQueryReply {
            result: Some(process_query_reply::Result::Empty(())),
            replies,
        } if replies == vec![vec![6]] => (),
        a => panic!("Expected replies: [[6]], got: {:?}", a),
    };

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn restore_after_complete_cycle() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    assert_resp_ok!(resp, InitReply, init_reply);

    let resp = client
        .process_event(ProcessEventRequest {
            source: Some(Path {
                segments: vec!["input".to_string()],
            }),
            event: vec![7],
        })
        .await
        .unwrap();
    assert_resp_ok!(resp, ProcessEventReply, process_event_reply);

    let state = fetch_state(&mut client).await;

    let resp = client.terminate(TerminateRequest {}).await.unwrap();
    assert_resp_ok!(resp, TerminateReply, terminate_reply);

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    let resp = client.restore(RestoreRequest { state }).await.unwrap();
    assert_resp_ok!(resp, RestoreReply, restore_reply);

    // Verify restored model state
    let resp = client
        .process_query(ProcessQueryRequest {
            source: Some(Path {
                segments: vec!["state_query".to_string()],
            }),
            // 0xf6 decodes as `null` in cbor which can be deserialized to `()`
            request: vec![0xf6],
        })
        .await
        .unwrap();
    match resp.into_inner() {
        ProcessQueryReply {
            result: Some(process_query_reply::Result::Empty(())),
            replies,
        } if replies == vec![vec![7]] => (),
        a => panic!("Expected replies: [[7]], got: {:?}", a),
    };

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn terminate_while_running() {
    let (mut client, signal, handle) = get_client(simple_bench).await;

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    let resp = client
        .init(InitRequest {
            time: Some(Timestamp::default()),
        })
        .await
        .unwrap();
    assert_resp_ok!(resp, InitReply, init_reply);

    // Schedule something so the simulation won't exit by itself.
    let resp = client
        .schedule_event(ScheduleEventRequest {
            source: Some(Path {
                segments: vec!["input".to_string()],
            }),
            event: vec![7],
            period: Some(prost_types::Duration {
                seconds: 1,
                ..Default::default()
            }),
            with_key: false,
            deadline: some_deadline_secs!(1, schedule_event_request),
        })
        .await
        .unwrap();
    assert_resp_ok!(resp, ScheduleEventReply, schedule_event_reply);

    let mut task_client = client.clone();

    // Request run concurrently.
    let task_handle = tokio::spawn(async move {
        let resp = task_client.run(RunRequest {}).await.unwrap();
        assert_resp_err!(resp, RunReply, run_reply, ErrorCode::SimulationTerminated);
    });

    // Let it run a bit.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let resp = client.terminate(TerminateRequest {}).await.unwrap();
    assert_resp_ok!(resp, TerminateReply, terminate_reply);

    // let task_result = task_handle.await;
    assert!(task_handle.await.is_ok());

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}

/// Helper executing save request on the client.
async fn fetch_state(client: &mut SimulationClient<tonic::transport::Channel>) -> Vec<u8> {
    let resp = client.save(SaveRequest {}).await.unwrap();
    match resp.into_inner() {
        SaveReply {
            result: Some(save_reply::Result::State(v)),
        } => v,
        a => panic!("Expected saved state, got: {:?}", a),
    }
}
