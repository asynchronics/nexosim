//! Simulation server.

use std::future::Future;
use std::net::SocketAddr;
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::{Mutex, RwLock};

use serde::de::DeserializeOwned;
use tonic::{transport::Server, Request, Response, Status};

use crate::registry::EndpointRegistry;
use crate::simulation::{Simulation, SimulationError};

use super::codegen::simulation::*;
use super::key_registry::KeyRegistry;
use super::services::InitService;
use super::services::{ControllerService, MonitorService, SchedulerService};

/// Runs a simulation from a network server.
///
/// The first argument is a closure that takes an initialization configuration
/// and is called every time the simulation is (re)started by the remote client.
/// It must create a new simulation, complemented by a registry that exposes the
/// public event and query interface.
pub fn run<F, I>(sim_gen: F, addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(I) -> Result<(Simulation, EndpointRegistry), SimulationError> + Send + 'static,
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
    F: FnMut(I) -> Result<(Simulation, EndpointRegistry), SimulationError> + Send + 'static,
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
    F: FnMut(I) -> Result<(Simulation, EndpointRegistry), SimulationError> + Send + 'static,
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
    F: FnMut(I) -> Result<(Simulation, EndpointRegistry), SimulationError> + Send + 'static,
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

struct GrpcSimulationService {
    init_service: Mutex<InitService>,
    controller_service: Arc<Mutex<ControllerService>>,
    monitor_service: Arc<RwLock<MonitorService>>,
    scheduler_service: Mutex<SchedulerService>,
}

impl GrpcSimulationService {
    /// Creates a new `GrpcSimulationService` without any active simulation.
    ///
    /// The argument is a closure that takes an initialization configuration and
    /// is called every time the simulation is (re)started by the remote client.
    /// It must create a new simulation, complemented by a registry that exposes
    /// the public event and query interface.
    pub(crate) fn new<F, I>(sim_gen: F) -> Self
    where
        F: FnMut(I) -> Result<(Simulation, EndpointRegistry), SimulationError> + Send + 'static,
        I: DeserializeOwned,
    {
        Self {
            init_service: Mutex::new(InitService::new(sim_gen)),
            controller_service: Arc::new(Mutex::new(ControllerService::NotStarted)),
            monitor_service: Arc::new(RwLock::new(MonitorService::NotStarted)),
            scheduler_service: Mutex::new(SchedulerService::NotStarted),
        }
    }

    /// Executes a method of the controller service.
    async fn execute_controller_fn<T, U, F>(&self, request: T, f: F) -> Result<Response<U>, Status>
    where
        T: Send + 'static,
        U: Send + 'static,
        F: Fn(&mut ControllerService, T) -> U + Send + 'static,
    {
        let controller = self.controller_service.clone();
        // May block.
        let res = tokio::task::spawn_blocking(move || f(&mut controller.lock().unwrap(), request))
            .await
            .unwrap();

        Ok(Response::new(res))
    }

    /// Executes a read method of the monitor service.
    async fn execute_monitor_read_fn<T, U, F>(
        &self,
        request: T,
        f: F,
    ) -> Result<Response<U>, Status>
    where
        T: Send + 'static,
        U: Send + 'static,
        F: Fn(&MonitorService, T) -> U + Send + 'static,
    {
        let monitor = self.monitor_service.clone();
        // May block.
        let res = tokio::task::spawn_blocking(move || f(&monitor.read().unwrap(), request))
            .await
            .unwrap();

        Ok(Response::new(res))
    }

    /// Executes a write method of the monitor service.
    async fn execute_monitor_write_fn<T, U, F>(
        &self,
        request: T,
        f: F,
    ) -> Result<Response<U>, Status>
    where
        T: Send + 'static,
        U: Send + 'static,
        F: Fn(&mut MonitorService, T) -> U + Send + 'static,
    {
        let monitor = self.monitor_service.clone();
        // May block.
        let res = tokio::task::spawn_blocking(move || f(&mut monitor.write().unwrap(), request))
            .await
            .unwrap();

        Ok(Response::new(res))
    }

    /// Executes a method of the scheduler service.
    // For some reason clippy emits a warning when generic `Response<U>` is
    // used, while not complaining with a concrete type.
    #[allow(clippy::result_large_err)]
    fn execute_scheduler_fn<T, U, F>(&self, request: T, f: F) -> Result<Response<U>, Status>
    where
        F: Fn(&mut SchedulerService, T) -> U,
    {
        Ok(Response::new(f(
            &mut self.scheduler_service.lock().unwrap(),
            request,
        )))
    }
}

#[tonic::async_trait]
impl simulation_server::Simulation for GrpcSimulationService {
    async fn init(&self, request: Request<InitRequest>) -> Result<Response<InitReply>, Status> {
        let request = request.into_inner();

        let (reply, bench) = self.init_service.lock().unwrap().init(request);

        if let Some((simulation, scheduler, endpoint_registry)) = bench {
            let event_source_registry = Arc::new(endpoint_registry.event_source_registry);
            let query_source_registry = endpoint_registry.query_source_registry;
            let event_sink_registry = endpoint_registry.event_sink_registry;

            *self.controller_service.lock().unwrap() = ControllerService::Started {
                simulation,
                event_source_registry: event_source_registry.clone(),
                query_source_registry,
            };
            *self.monitor_service.write().unwrap() = MonitorService::Started {
                event_sink_registry,
            };
            *self.scheduler_service.lock().unwrap() = SchedulerService::Started {
                scheduler,
                event_source_registry,
                key_registry: KeyRegistry::default(),
            };
        }

        Ok(Response::new(reply))
    }
    async fn terminate(
        &self,
        _request: Request<TerminateRequest>,
    ) -> Result<Response<TerminateReply>, Status> {
        *self.controller_service.lock().unwrap() = ControllerService::NotStarted;
        *self.monitor_service.write().unwrap() = MonitorService::NotStarted;
        *self.scheduler_service.lock().unwrap() = SchedulerService::NotStarted;

        Ok(Response::new(TerminateReply {
            result: Some(terminate_reply::Result::Empty(())),
        }))
    }
    async fn halt(&self, request: Request<HaltRequest>) -> Result<Response<HaltReply>, Status> {
        let request = request.into_inner();

        self.execute_scheduler_fn(request, SchedulerService::halt)
    }
    async fn time(&self, request: Request<TimeRequest>) -> Result<Response<TimeReply>, Status> {
        let request = request.into_inner();

        self.execute_scheduler_fn(request, SchedulerService::time)
    }
    async fn step(&self, request: Request<StepRequest>) -> Result<Response<StepReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::step)
            .await
    }
    async fn step_until(
        &self,
        request: Request<StepUntilRequest>,
    ) -> Result<Response<StepUntilReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::step_until)
            .await
    }
    async fn step_unbounded(
        &self,
        request: Request<StepUnboundedRequest>,
    ) -> Result<Response<StepUnboundedReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::step_unbounded)
            .await
    }
    async fn schedule_event(
        &self,
        request: Request<ScheduleEventRequest>,
    ) -> Result<Response<ScheduleEventReply>, Status> {
        let request = request.into_inner();

        self.execute_scheduler_fn(request, SchedulerService::schedule_event)
    }
    async fn cancel_event(
        &self,
        request: Request<CancelEventRequest>,
    ) -> Result<Response<CancelEventReply>, Status> {
        let request = request.into_inner();

        self.execute_scheduler_fn(request, SchedulerService::cancel_event)
    }
    async fn list_event_sources(
        &self,
        request: Request<ListEventSourcesRequest>,
    ) -> Result<Response<ListEventSourcesReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::list_event_sources)
            .await
    }
    async fn get_event_source_schemas(
        &self,
        request: Request<GetEventSourceSchemasRequest>,
    ) -> Result<Response<GetEventSourceSchemasReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::get_event_source_schemas)
            .await
    }
    async fn list_query_sources(
        &self,
        request: Request<ListQuerySourcesRequest>,
    ) -> Result<Response<ListQuerySourcesReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::list_query_sources)
            .await
    }
    async fn get_query_source_schemas(
        &self,
        request: Request<GetQuerySourceSchemasRequest>,
    ) -> Result<Response<GetQuerySourceSchemasReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::get_query_source_schemas)
            .await
    }
    async fn list_event_sinks(
        &self,
        request: Request<ListEventSinksRequest>,
    ) -> Result<Response<ListEventSinksReply>, Status> {
        let request = request.into_inner();

        self.execute_monitor_read_fn(request, MonitorService::list_event_sinks)
            .await
    }
    async fn get_event_sink_schemas(
        &self,
        request: Request<GetEventSinkSchemasRequest>,
    ) -> Result<Response<GetEventSinkSchemasReply>, Status> {
        let request = request.into_inner();

        self.execute_monitor_read_fn(request, MonitorService::get_event_sink_schemas)
            .await
    }
    async fn process_event(
        &self,
        request: Request<ProcessEventRequest>,
    ) -> Result<Response<ProcessEventReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::process_event)
            .await
    }
    async fn process_query(
        &self,
        request: Request<ProcessQueryRequest>,
    ) -> Result<Response<ProcessQueryReply>, Status> {
        let request = request.into_inner();

        self.execute_controller_fn(request, ControllerService::process_query)
            .await
    }
    async fn read_events(
        &self,
        request: Request<ReadEventsRequest>,
    ) -> Result<Response<ReadEventsReply>, Status> {
        let request = request.into_inner();

        self.execute_monitor_read_fn(request, MonitorService::read_events)
            .await
    }
    async fn await_event(
        &self,
        request: Request<AwaitEventRequest>,
    ) -> Result<Response<AwaitEventReply>, Status> {
        let request = request.into_inner();

        self.execute_monitor_read_fn(request, MonitorService::await_event)
            .await
    }
    async fn open_sink(
        &self,
        request: Request<OpenSinkRequest>,
    ) -> Result<Response<OpenSinkReply>, Status> {
        let request = request.into_inner();

        self.execute_monitor_write_fn(request, MonitorService::open_sink)
            .await
    }
    async fn close_sink(
        &self,
        request: Request<CloseSinkRequest>,
    ) -> Result<Response<CloseSinkReply>, Status> {
        let request = request.into_inner();

        self.execute_monitor_write_fn(request, MonitorService::close_sink)
            .await
    }
}
