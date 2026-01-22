//! Simulation server.

use std::error;
use std::future::Future;
use std::net::SocketAddr;
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;

use serde::de::DeserializeOwned;
use tonic::transport::Server;

use crate::server::services::GrpcSimulationService;
use crate::simulation::SimInit;

use super::codegen::simulation::simulation_server;

/// Runs a simulation from a network server.
///
/// The first argument is a closure that takes an initialization configuration
/// and is called every time the simulation is (re)started by the remote client.
/// It must create a new simulation, complemented by a registry that exposes the
/// public event and query interface.
pub fn run<F, I>(sim_gen: F, addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(I) -> Result<SimInit, Box<dyn error::Error>> + Send + 'static,
    I: DeserializeOwned,
{
    run_service(GrpcSimulationService::new(sim_gen), addr, None)
}

/// Runs a simulation from a network server until a signal is received.
///
/// The first argument is a closure that takes an initialization configuration
/// and is called every time the simulation is (re)started by the remote client.
/// It must create a new simulation, complemented by a registry that exposes the
/// public event and query interface.
///
/// The server shuts down cleanly when the shutdown signal is received.
pub fn run_with_shutdown<F, I, S>(
    sim_gen: F,
    addr: SocketAddr,
    signal: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(I) -> Result<SimInit, Box<dyn error::Error>> + Send + 'static,
    I: DeserializeOwned,
    for<'a> S: Future<Output = ()> + 'a,
{
    run_service(
        GrpcSimulationService::new(sim_gen),
        addr,
        Some(Box::pin(signal)),
    )
}

/// Monomorphization of the network server.
///
/// Keeping this as a separate monomorphized fragment can even triple
/// compilation speed for incremental release builds.
fn run_service(
    service: GrpcSimulationService,
    addr: SocketAddr,
    signal: Option<Pin<Box<dyn Future<Output = ()>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()?;

    rt.block_on(async move {
        let service =
            Server::builder().add_service(simulation_server::SimulationServer::new(service));

        match signal {
            Some(signal) => service.serve_with_shutdown(addr, signal).await?,
            None => service.serve(addr).await?,
        };

        Ok(())
    })
}

/// Runs a simulation locally from a Unix Domain Sockets server.
///
/// The first argument is a closure that takes an initialization configuration
/// and is called every time the simulation is (re)started by the remote client.
/// It must create a new simulation, complemented by a registry that exposes the
/// public event and query interface.
#[cfg(unix)]
pub fn run_local<F, I, P>(sim_gen: F, path: P) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(I) -> Result<SimInit, Box<dyn error::Error>> + Send + 'static,
    I: DeserializeOwned,
    P: AsRef<Path>,
{
    let path = path.as_ref();
    run_local_service(GrpcSimulationService::new(sim_gen), path, None)
}

/// Runs a simulation locally from a Unix Domain Sockets server until a signal
/// is received.
///
/// The first argument is a closure that takes an initialization configuration
/// and is called every time the simulation is (re)started by the remote client.
/// It must create a new simulation, complemented by a registry that exposes the
/// public event and query interface.
///
/// The server shuts down cleanly when the shutdown signal is received.
#[cfg(unix)]
pub fn run_local_with_shutdown<F, I, P, S>(
    sim_gen: F,
    path: P,
    signal: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(I) -> Result<SimInit, Box<dyn error::Error>> + Send + 'static,
    I: DeserializeOwned,
    P: AsRef<Path>,
    for<'a> S: Future<Output = ()> + 'a,
{
    let path = path.as_ref();
    run_local_service(
        GrpcSimulationService::new(sim_gen),
        path,
        Some(Box::pin(signal)),
    )
}

/// Monomorphization of the Unix Domain Sockets server.
///
/// Keeping this as a separate monomorphized fragment can even triple
/// compilation speed for incremental release builds.
#[cfg(unix)]
fn run_local_service(
    service: GrpcSimulationService,
    path: &Path,
    signal: Option<Pin<Box<dyn Future<Output = ()>>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;
    use std::io;
    use std::os::unix::fs::FileTypeExt;

    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;

    // Unlink the socket if it already exists to prevent an `AddrInUse` error.
    match fs::metadata(path) {
        // The path is valid: make sure it actually points to a socket.
        Ok(socket_meta) => {
            if !socket_meta.file_type().is_socket() {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "the specified path points to an existing non-socket file",
                )));
            }

            fs::remove_file(path)?;
        }
        // Nothing to do: the socket does not exist yet.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        // We don't have permission to use the socket.
        Err(e) => return Err(Box::new(e)),
    }

    // (Re-)Create the socket.
    fs::create_dir_all(path.parent().unwrap())?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()?;

    rt.block_on(async move {
        let uds = UnixListener::bind(path)?;
        let uds_stream = UnixListenerStream::new(uds);

        let service =
            Server::builder().add_service(simulation_server::SimulationServer::new(service));

        match signal {
            Some(signal) => {
                service
                    .serve_with_incoming_shutdown(uds_stream, signal)
                    .await?
            }
            None => service.serve_with_incoming(uds_stream).await?,
        };

        Ok(())
    })
}
