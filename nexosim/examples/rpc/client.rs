use nexosim::client::{
    BuildRequest, InitRequest, Path, ProcessEventRequest, ProcessQueryReply, ProcessQueryRequest,
    SimulationClient, TerminateRequest, encode_payload,
};

const HOST: &str = "127.0.0.1";
const PORT: u16 = 8888;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut client = SimulationClient::connect(format!("http://{HOST}:{PORT}"))
        .await
        .unwrap();

    client.build(BuildRequest { cfg: vec![0] }).await.unwrap();

    client
        .init(InitRequest {
            time: Some(prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            }),
        })
        .await
        .unwrap();

    client
        .process_event(ProcessEventRequest {
            source: Some(Path {
                segments: vec!["store".to_string()],
            }),
            event: encode_payload(&3).unwrap(),
        })
        .await
        .unwrap();

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
