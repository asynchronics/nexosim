use nexosim::client::{
    BuildReply, BuildRequest, InitReply, InitRequest, Path, ProcessEventReply, ProcessEventRequest,
    ProcessQueryReply, ProcessQueryRequest, SimulationClient, TerminateRequest, build_reply,
    encode_payload, init_reply, process_event_reply,
};

const HOST: &str = "127.0.0.1";
const PORT: u16 = 8888;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut client = SimulationClient::connect(format!("http://{HOST}:{PORT}"))
        .await
        .unwrap();

    let resp = client
        .build(BuildRequest {
            cfg: encode_payload(&0).unwrap(),
        })
        .await
        .unwrap();
    assert!(matches!(
        resp.into_inner(),
        BuildReply {
            result: Some(build_reply::Result::Empty(_))
        }
    ));

    let resp = client
        .init(InitRequest {
            time: Some(prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            }),
        })
        .await
        .unwrap();
    assert!(matches!(
        resp.into_inner(),
        InitReply {
            result: Some(init_reply::Result::Empty(_))
        }
    ));

    let resp = client
        .process_event(ProcessEventRequest {
            source: Some(Path {
                segments: vec!["store".to_string()],
            }),
            event: encode_payload(&3).unwrap(),
        })
        .await
        .unwrap();
    assert!(matches!(
        resp.into_inner(),
        ProcessEventReply {
            result: Some(process_event_reply::Result::Empty(_))
        }
    ));

    let resp = client
        .process_query(ProcessQueryRequest {
            source: Some(Path {
                segments: vec!["load".to_string()],
            }),
            request: encode_payload(&()).unwrap(),
        })
        .await
        .unwrap();
    assert!(matches!(
        resp.into_inner(),
        ProcessQueryReply { replies, .. } if replies == [encode_payload(&3).unwrap()]
    ));

    client.terminate(TerminateRequest {}).await.unwrap();
}
