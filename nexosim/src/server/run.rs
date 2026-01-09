//! Simulation server.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
#[cfg(unix)]
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::RwLock as TokioRwLock;
use tonic::{Request, Response, Status, transport::Server};

use crate::endpoints::Endpoints;
use crate::simulation::{SimInit, Simulation};

use super::codegen::simulation::*;
use super::key_registry::KeyRegistry;
use super::services::{
    ControllerService, InitService, InspectorService, MonitorService, SchedulerService,
    monitor_service_read_event,
};

/// Runs a simulation from a network server.
///
/// The first argument is a closure that takes an initialization configuration
/// and is called every time the simulation is (re)started by the remote client.
/// It must create a new simulation, complemented by a registry that exposes the
/// public event and query interface.
pub fn run<F, I>(sim_gen: F, addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(I) -> SimInit + Send + 'static,
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
    F: FnMut(I) -> SimInit + Send + 'static,
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
    F: FnMut(I) -> SimInit + Send + 'static,
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
    F: FnMut(I) -> SimInit + Send + 'static,
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

/// The global gRPC service.
///
/// In order to allow concurrent non-mutating requests, all such requests are
/// routed to an `InspectorService` which is under an async RW lock. The only
/// time it is accessed in writing mode is during initialization and
/// termination.
///
/// Mutable resources that can be managed through non-blocking or async calls,
/// namely the `KeyRegistry` (held by `SchedulerService`) and the
/// `EventSinkRegistry` (held by `MonitorService`), are bundled with all
/// necessary immutable resources in a service object held under an async mutex.
///
/// In the case of the `Simulation` instance, managed by `ControllerService`, a
/// standard mutex is used and wrapped in an `Arc`. This allows mutating
/// blocking requests to the simulation object to be spawned on a separate
/// thread with `tokio::task::spawn_blocking`.
struct GrpcSimulationService {
    init_service: TokioMutex<InitService>,
    controller_service: Arc<Mutex<ControllerService>>,
    scheduler_service: TokioMutex<SchedulerService>,
    monitor_service: TokioMutex<MonitorService>,
    inspector_service: TokioRwLock<InspectorService>,
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
        F: FnMut(I) -> SimInit + Send + 'static,
        I: DeserializeOwned,
    {
        Self {
            init_service: TokioMutex::new(InitService::new(sim_gen)),
            controller_service: Arc::new(Mutex::new(ControllerService::Halted)),
            scheduler_service: TokioMutex::new(SchedulerService::Halted),
            monitor_service: TokioMutex::new(MonitorService::Halted),
            inspector_service: TokioRwLock::new(InspectorService::Halted),
        }
    }

    /// Executes a method of the controller service.
    async fn execute_controller_fn<T, U, F>(&self, request: T, f: F) -> U
    where
        T: Send + 'static,
        U: Send + 'static,
        F: Fn(&mut ControllerService, T) -> U + Send + 'static,
    {
        let controller = self.controller_service.clone();

        // May block!
        tokio::task::spawn_blocking(move || f(&mut controller.lock().unwrap(), request))
            .await
            .unwrap()
    }

    async fn start_services(
        &self,
        simulation: Simulation,
        endpoint_registry: Endpoints,
        cfg: Vec<u8>,
    ) {
        let scheduler = simulation.scheduler();

        let (
            event_sink_registry,
            event_sink_info_registry,
            event_source_registry,
            query_source_registry,
        ) = endpoint_registry.into_parts();
        let event_source_registry = Arc::new(event_source_registry);
        let query_source_registry = Arc::new(query_source_registry);

        *self.controller_service.lock().unwrap() = ControllerService::Started {
            cfg,
            simulation,
            event_source_registry: event_source_registry.clone(),
            query_source_registry: query_source_registry.clone(),
        };
        *self.inspector_service.write().await = InspectorService::Started {
            scheduler: scheduler.clone(),
            event_sink_info_registry,
            event_source_registry: event_source_registry.clone(),
            query_source_registry,
        };
        *self.monitor_service.lock().await = MonitorService::Started {
            event_sink_registry,
        };
        *self.scheduler_service.lock().await = SchedulerService::Started {
            scheduler,
            event_source_registry,
            key_registry: KeyRegistry::default(),
        };
    }
}

#[tonic::async_trait]
impl simulation_server::Simulation for GrpcSimulationService {
    // Terminate.
    async fn terminate(
        &self,
        _request: Request<TerminateRequest>,
    ) -> Result<Response<TerminateReply>, Status> {
        *self.controller_service.lock().unwrap() = ControllerService::Halted;
        *self.inspector_service.write().await = InspectorService::Halted;
        *self.monitor_service.lock().await = MonitorService::Halted;
        *self.scheduler_service.lock().await = SchedulerService::Halted;

        Ok(Response::new(TerminateReply {
            result: Some(terminate_reply::Result::Empty(())),
        }))
    }

    // Init service.
    async fn init(&self, request: Request<InitRequest>) -> Result<Response<InitReply>, Status> {
        let request = request.into_inner();
        let (reply, bench) = self.init_service.lock().await.init(request);

        if let Some((simulation, endpoint_registry, cfg)) = bench {
            self.start_services(simulation, endpoint_registry, cfg)
                .await;
        }

        Ok(Response::new(reply))
    }
    async fn restore(
        &self,
        request: Request<RestoreRequest>,
    ) -> Result<Response<RestoreReply>, Status> {
        let request = request.into_inner();
        let (reply, bench) = self.init_service.lock().await.restore(request);

        if let Some((simulation, endpoint_registry, cfg)) = bench {
            self.start_services(simulation, endpoint_registry, cfg)
                .await;
        }

        Ok(Response::new(reply))
    }

    // Controller service.
    async fn save(&self, request: Request<SaveRequest>) -> Result<Response<SaveReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .execute_controller_fn(request, ControllerService::save)
            .await;

        Ok(Response::new(SaveReply {
            result: Some(match reply {
                Ok(state) => save_reply::Result::State(state),
                Err(e) => save_reply::Result::Error(e),
            }),
        }))
    }
    async fn step(&self, request: Request<StepRequest>) -> Result<Response<StepReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .execute_controller_fn(request, ControllerService::step)
            .await;

        Ok(Response::new(StepReply {
            result: Some(match reply {
                Ok(timestamp) => step_reply::Result::Time(timestamp),
                Err(e) => step_reply::Result::Error(e),
            }),
        }))
    }
    async fn step_until(
        &self,
        request: Request<StepUntilRequest>,
    ) -> Result<Response<StepUntilReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .execute_controller_fn(request, ControllerService::step_until)
            .await;

        Ok(Response::new(StepUntilReply {
            result: Some(match reply {
                Ok(timestamp) => step_until_reply::Result::Time(timestamp),
                Err(error) => step_until_reply::Result::Error(error),
            }),
        }))
    }
    async fn step_unbounded(
        &self,
        request: Request<StepUnboundedRequest>,
    ) -> Result<Response<StepUnboundedReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .execute_controller_fn(request, ControllerService::step_unbounded)
            .await;

        Ok(Response::new(StepUnboundedReply {
            result: Some(match reply {
                Ok(timestamp) => step_unbounded_reply::Result::Time(timestamp),
                Err(error) => step_unbounded_reply::Result::Error(error),
            }),
        }))
    }
    async fn process_event(
        &self,
        request: Request<ProcessEventRequest>,
    ) -> Result<Response<ProcessEventReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .execute_controller_fn(request, ControllerService::process_event)
            .await;

        Ok(Response::new(ProcessEventReply {
            result: Some(match reply {
                Ok(()) => process_event_reply::Result::Empty(()),
                Err(error) => process_event_reply::Result::Error(error),
            }),
        }))
    }
    async fn process_query(
        &self,
        request: Request<ProcessQueryRequest>,
    ) -> Result<Response<ProcessQueryReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .execute_controller_fn(request, ControllerService::process_query)
            .await;

        Ok(Response::new(match reply {
            Ok(replies) => ProcessQueryReply {
                replies,
                result: Some(process_query_reply::Result::Empty(())),
            },
            Err(error) => ProcessQueryReply {
                replies: Vec::new(),
                result: Some(process_query_reply::Result::Error(error)),
            },
        }))
    }

    // Scheduler service.
    async fn schedule_event(
        &self,
        request: Request<ScheduleEventRequest>,
    ) -> Result<Response<ScheduleEventReply>, Status> {
        let request = request.into_inner();
        let reply = self.scheduler_service.lock().await.schedule_event(request);

        Ok(Response::new(ScheduleEventReply {
            result: Some(match reply {
                Ok(Some(key_id)) => {
                    let (subkey1, subkey2) = key_id.into_raw_parts();
                    schedule_event_reply::Result::Key(EventKey {
                        subkey1: subkey1
                            .try_into()
                            .expect("action key index is too large to be serialized"),
                        subkey2,
                    })
                }
                Ok(None) => schedule_event_reply::Result::Empty(()),
                Err(error) => schedule_event_reply::Result::Error(error),
            }),
        }))
    }
    async fn cancel_event(
        &self,
        request: Request<CancelEventRequest>,
    ) -> Result<Response<CancelEventReply>, Status> {
        let request = request.into_inner();
        let reply = self.scheduler_service.lock().await.cancel_event(request);

        Ok(Response::new(CancelEventReply {
            result: Some(match reply {
                Ok(()) => cancel_event_reply::Result::Empty(()),
                Err(error) => cancel_event_reply::Result::Error(error),
            }),
        }))
    }

    // Inspector service.
    async fn time(&self, request: Request<TimeRequest>) -> Result<Response<TimeReply>, Status> {
        let request = request.into_inner();

        Ok(Response::new(TimeReply {
            result: Some(match self.inspector_service.read().await.time(request) {
                Ok(timestamp) => time_reply::Result::Time(timestamp),
                Err(e) => time_reply::Result::Error(e),
            }),
        }))
    }
    async fn halt(&self, request: Request<HaltRequest>) -> Result<Response<HaltReply>, Status> {
        let request = request.into_inner();

        Ok(Response::new(HaltReply {
            result: Some(match self.inspector_service.read().await.halt(request) {
                Ok(()) => halt_reply::Result::Empty(()),
                Err(e) => halt_reply::Result::Error(e),
            }),
        }))
    }
    async fn list_event_sources(
        &self,
        request: Request<ListEventSourcesRequest>,
    ) -> Result<Response<ListEventSourcesReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .inspector_service
            .read()
            .await
            .list_event_sources(request);

        Ok(Response::new(match reply {
            Ok(source_names) => ListEventSourcesReply {
                source_names,
                result: Some(list_event_sources_reply::Result::Empty(())),
            },
            Err(e) => ListEventSourcesReply {
                source_names: Vec::new(),
                result: Some(list_event_sources_reply::Result::Error(e)),
            },
        }))
    }
    async fn get_event_source_schemas(
        &self,
        request: Request<GetEventSourceSchemasRequest>,
    ) -> Result<Response<GetEventSourceSchemasReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .inspector_service
            .read()
            .await
            .get_event_source_schemas(request);

        Ok(Response::new(match reply {
            Ok(schemas) => GetEventSourceSchemasReply {
                schemas,
                result: Some(get_event_source_schemas_reply::Result::Empty(())),
            },
            Err(e) => GetEventSourceSchemasReply {
                schemas: HashMap::new(),
                result: Some(get_event_source_schemas_reply::Result::Error(e)),
            },
        }))
    }
    async fn list_query_sources(
        &self,
        request: Request<ListQuerySourcesRequest>,
    ) -> Result<Response<ListQuerySourcesReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .inspector_service
            .read()
            .await
            .list_query_sources(request);

        Ok(Response::new(match reply {
            Ok(source_names) => ListQuerySourcesReply {
                source_names,
                result: Some(list_query_sources_reply::Result::Empty(())),
            },
            Err(e) => ListQuerySourcesReply {
                source_names: Vec::new(),
                result: Some(list_query_sources_reply::Result::Error(e)),
            },
        }))
    }
    async fn get_query_source_schemas(
        &self,
        request: Request<GetQuerySourceSchemasRequest>,
    ) -> Result<Response<GetQuerySourceSchemasReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .inspector_service
            .read()
            .await
            .get_query_source_schemas(request);

        Ok(Response::new(match reply {
            Ok(schemas) => GetQuerySourceSchemasReply {
                schemas,
                result: Some(get_query_source_schemas_reply::Result::Empty(())),
            },
            Err(e) => GetQuerySourceSchemasReply {
                schemas: HashMap::new(),
                result: Some(get_query_source_schemas_reply::Result::Error(e)),
            },
        }))
    }
    async fn list_event_sinks(
        &self,
        request: Request<ListEventSinksRequest>,
    ) -> Result<Response<ListEventSinksReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .inspector_service
            .read()
            .await
            .list_event_sinks(request);

        Ok(Response::new(match reply {
            Ok(sink_names) => ListEventSinksReply {
                sink_names,
                result: Some(list_event_sinks_reply::Result::Empty(())),
            },
            Err(e) => ListEventSinksReply {
                sink_names: Vec::new(),
                result: Some(list_event_sinks_reply::Result::Error(e)),
            },
        }))
    }
    async fn get_event_sink_schemas(
        &self,
        request: Request<GetEventSinkSchemasRequest>,
    ) -> Result<Response<GetEventSinkSchemasReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .inspector_service
            .read()
            .await
            .get_event_sink_schemas(request);

        Ok(Response::new(match reply {
            Ok(schemas) => GetEventSinkSchemasReply {
                schemas,
                result: Some(get_event_sink_schemas_reply::Result::Empty(())),
            },
            Err(e) => GetEventSinkSchemasReply {
                schemas: HashMap::new(),
                result: Some(get_event_sink_schemas_reply::Result::Error(e)),
            },
        }))
    }

    // Monitor service.
    async fn try_read_events(
        &self,
        request: Request<TryReadEventsRequest>,
    ) -> Result<Response<TryReadEventsReply>, Status> {
        let request = request.into_inner();

        let reply = self.monitor_service.lock().await.try_read_events(request);

        Ok(Response::new(match reply {
            Ok(events) => TryReadEventsReply {
                events,
                result: Some(try_read_events_reply::Result::Empty(())),
            },
            Err(error) => TryReadEventsReply {
                events: Vec::new(),
                result: Some(try_read_events_reply::Result::Error(error)),
            },
        }))
    }
    async fn read_event(
        &self,
        request: Request<ReadEventRequest>,
    ) -> Result<Response<ReadEventReply>, Status> {
        let request = request.into_inner();

        let reply = monitor_service_read_event(&self.monitor_service, request).await;

        Ok(Response::new(ReadEventReply {
            result: Some(match reply {
                Ok(event) => read_event_reply::Result::Event(event),
                Err(error) => read_event_reply::Result::Error(error),
            }),
        }))
    }
    async fn enable_sink(
        &self,
        request: Request<EnableSinkRequest>,
    ) -> Result<Response<EnableSinkReply>, Status> {
        let request = request.into_inner();
        let reply = self.monitor_service.lock().await.enable_sink(request);

        Ok(Response::new(EnableSinkReply {
            result: Some(match reply {
                Ok(()) => enable_sink_reply::Result::Empty(()),
                Err(e) => enable_sink_reply::Result::Error(e),
            }),
        }))
    }
    async fn disable_sink(
        &self,
        request: Request<DisableSinkRequest>,
    ) -> Result<Response<DisableSinkReply>, Status> {
        let request = request.into_inner();
        let reply = self.monitor_service.lock().await.disable_sink(request);

        Ok(Response::new(DisableSinkReply {
            result: Some(match reply {
                Ok(()) => disable_sink_reply::Result::Empty(()),
                Err(e) => disable_sink_reply::Result::Error(e),
            }),
        }))
    }
}
