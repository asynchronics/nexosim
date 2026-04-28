use std::time::Duration;

use tokio::sync::oneshot::Sender;
use tokio::task::JoinHandle;

use nexosim::simulation::SimInit;

pub(crate) mod grpc_client {
    include!("../codegen/simulation.v1.rs");
}
use grpc_client::simulation_client::SimulationClient;

/// Helper macro to generate namespaced deadline argument for gRPC requests.
#[macro_export]
macro_rules! some_deadline_secs {
    ($seconds:expr, $request:ident) => {
        Some($request::Deadline::Duration(prost_types::Duration {
            seconds: $seconds,
            ..Default::default()
        }))
    };
}

fn get_free_port() -> u16 {
    let socket = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    socket.local_addr().unwrap().port()
}

/// Creates a client to a background-running gRPC server.
///
/// Returns (gRPC client, shudtown signal, server thread handle)
pub(crate) async fn get_client<F, I>(
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

    let client = SimulationClient::connect(format!("http://127.0.0.1:{port}"))
        .await
        .unwrap();

    (client, tx, handle)
}
