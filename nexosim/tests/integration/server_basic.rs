use prost_types::Timestamp;
use serde::{Deserialize, Serialize};

use nexosim::model::{Context, Model};
use nexosim::ports::QuerySource;
use nexosim::simulation::{Mailbox, SimInit};

use super::server_utils::get_client;
use super::server_utils::grpc_client::{
    BuildReply, BuildRequest, Error, ErrorCode, InitReply, InitRequest, Path, ProcessQueryReply,
    ProcessQueryRequest, StepUntilRequest, TerminateReply, TerminateRequest, build_reply,
    init_reply, process_query_reply, step_until_request, terminate_reply,
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

#[derive(Serialize, Deserialize)]
struct SimpleModel;
#[Model]
impl SimpleModel {
    async fn query(&mut self, value: i64, cx: &Context<Self>) -> i64 {
        value * cx.time().as_secs()
    }
}

fn simple_bench(_: u8) -> Result<SimInit, Box<dyn std::error::Error>> {
    let simple = SimpleModel;
    let simple_mbox = Mailbox::new();

    let mut bench = SimInit::new();

    QuerySource::new()
        .connect(SimpleModel::query, &simple_mbox)
        .bind_endpoint(&mut bench, "query")?;

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
            deadline: some_deadline_secs!(1, step_until_request),
        })
        .await
        .unwrap();

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_err!(resp, BuildReply, build_reply, ErrorCode::BenchAlreadyBuilt);

    // Verify that the simulation is sill alive and at a right ts.
    let resp = client
        .process_query(ProcessQueryRequest {
            source: Some(Path {
                segments: vec!["query".to_string()],
            }),
            request: vec![9],
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
                segments: vec!["query".to_string()],
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
async fn build_after_complete_cycle() {
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

    let resp = client.terminate(TerminateRequest {}).await.unwrap();
    assert_resp_ok!(resp, TerminateReply, terminate_reply);

    let resp = client.build(BuildRequest { cfg: vec![0] }).await.unwrap();
    assert_resp_ok!(resp, BuildReply, build_reply);

    // Shutdown.
    signal.send(()).unwrap();
    handle.await.unwrap();
}
