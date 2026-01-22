mod build_service;
mod controller_service;
mod inspector_service;
mod monitor_service;
mod scheduler_service;

use std::error;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use prost_types::Timestamp;
use serde::de::DeserializeOwned;
use tai_time::MonotonicTime;
use tonic::{Request, Response, Status};

use super::codegen::simulation::*;
use super::key_registry::KeyRegistry;
use crate::endpoints::{EndpointError, Endpoints};
use crate::simulation::{
    BenchError, ExecutionError, SchedulingError, SimInit, Simulation, SimulationError,
};

pub(crate) use build_service::BuildService;
pub(crate) use controller_service::ControllerService;
pub(crate) use inspector_service::InspectorService;
pub(crate) use monitor_service::{MonitorService, monitor_service_read_event};
pub(crate) use scheduler_service::SchedulerService;

/// The global gRPC service.
///
/// In order to allow concurrent non-mutating requests, most such requests are
/// routed to an `InspectorService` which is under an RW lock. The only time it
/// is accessed in writing mode is during initialization and termination.
///
/// Mutable resources that can be managed through non-blocking or async calls,
/// namely the `KeyRegistry` (held by `SchedulerService`) and the
/// `EventSinkRegistry` (held by `MonitorService`), are bundled with all
/// necessary immutable resources in a service object held under a mutex.
///
/// In the case of the `Simulation` instance, managed by `ControllerService`, a
/// mutex is wrapped in an `Arc`. This allows mutating blocking requests to the
/// simulation object to be spawned on a separate thread with
/// `tokio::task::spawn_blocking`.
pub(crate) struct GrpcSimulationService {
    build_service: Mutex<BuildService>,
    controller_service: Arc<Mutex<ControllerService>>,
    scheduler_service: Mutex<SchedulerService>,
    monitor_service: Mutex<MonitorService>,
    inspector_service: RwLock<InspectorService>,
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
        F: FnMut(I) -> Result<SimInit, Box<dyn error::Error>> + Send + 'static,
        I: DeserializeOwned,
    {
        Self {
            build_service: Mutex::new(BuildService::new(sim_gen)),
            controller_service: Arc::new(Mutex::new(ControllerService::Halted)),
            scheduler_service: Mutex::new(SchedulerService::Halted),
            monitor_service: Mutex::new(MonitorService::Halted),
            inspector_service: RwLock::new(InspectorService::Halted),
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

    fn start_services(&self, simulation: Simulation, endpoint_registry: Endpoints) {
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
            simulation,
            event_source_registry: event_source_registry.clone(),
            query_source_registry: query_source_registry.clone(),
        };
        *self.inspector_service.write().unwrap() = InspectorService::Started {
            event_sink_info_registry,
            event_source_registry: event_source_registry.clone(),
            query_source_registry,
        };
        *self.monitor_service.lock().unwrap() = MonitorService::Started {
            event_sink_registry,
        };
        *self.scheduler_service.lock().unwrap() = SchedulerService::Started {
            scheduler,
            event_source_registry,
            key_registry: KeyRegistry::default(),
        };
    }
}

/// Transforms an error code and a message into a Protobuf error.
fn to_error(code: ErrorCode, message: impl Into<String>) -> Error {
    Error {
        code: code as i32,
        message: message.into(),
    }
}

/// An error returned when no simulation has been initialized/restored or the
/// simulation is halted.
fn simulation_not_started_error() -> Error {
    to_error(
        ErrorCode::SimulationNotStarted,
        "the simulation has not been started",
    )
}

/// Transform an `ExecutionError` into a Protobuf error.
fn from_execution_error(error: ExecutionError) -> Error {
    let error_code = match error {
        ExecutionError::Deadlock(_) => ErrorCode::SimulationDeadlock,
        ExecutionError::MessageLoss(_) => ErrorCode::SimulationMessageLoss,
        ExecutionError::NoRecipient { .. } => ErrorCode::SimulationNoRecipient,
        ExecutionError::Panic { .. } => ErrorCode::SimulationPanic,
        ExecutionError::Timeout => ErrorCode::SimulationTimeout,
        ExecutionError::OutOfSync(_) => ErrorCode::SimulationOutOfSync,
        ExecutionError::BadQuery => ErrorCode::SimulationBadQuery,
        ExecutionError::Halted => ErrorCode::SimulationNotStarted,
        ExecutionError::Terminated => ErrorCode::SimulationTerminated,
        ExecutionError::InvalidDeadline(_) => ErrorCode::InvalidDeadline,
        ExecutionError::InvalidEventId(_) => ErrorCode::EventSourceNotFound,
        ExecutionError::InvalidQueryId(_) => ErrorCode::QuerySourceNotFound,
        ExecutionError::SaveError(_) => ErrorCode::SaveError,
        ExecutionError::RestoreError(_) => ErrorCode::RestoreError,
    };

    let error_message = error.to_string();

    to_error(error_code, error_message)
}

/// Transform a `SchedulingError` into a Protobuf error.
fn from_scheduling_error(error: SchedulingError) -> Error {
    let error_code = match error {
        SchedulingError::InvalidScheduledTime => ErrorCode::InvalidDeadline,
        SchedulingError::NullRepetitionPeriod => ErrorCode::InvalidPeriod,
    };

    let error_message = error.to_string();

    to_error(error_code, error_message)
}

/// Transform a `BenchError` into a Protobuf error.
fn from_bench_error(error: BenchError) -> Error {
    let error_code = match error {
        BenchError::DuplicateEventSource(_) => ErrorCode::DuplicateEventSource,
        BenchError::DuplicateQuerySource(_) => ErrorCode::DuplicateQuerySource,
        BenchError::DuplicateEventSink(_) => ErrorCode::DuplicateEventSink,
    };

    let error_message = error.to_string();

    to_error(error_code, error_message)
}

/// Transform a `SimulationError` into a Protobuf error.
fn from_simulation_error(error: SimulationError) -> Error {
    match error {
        SimulationError::ExecutionError(e) => from_execution_error(e),
        SimulationError::SchedulingError(e) => from_scheduling_error(e),
        SimulationError::BenchError(e) => from_bench_error(e),
    }
}

fn from_endpoint_error(error: EndpointError) -> Error {
    let error_code = match error {
        EndpointError::EventSourceNotFound { .. } => ErrorCode::EventSourceNotFound,
        EndpointError::QuerySourceNotFound { .. } => ErrorCode::QuerySourceNotFound,
        EndpointError::EventSinkNotFound { .. } => ErrorCode::SinkNotFound,
        EndpointError::InvalidEventSourceType { .. } => ErrorCode::InvalidEventType,
        EndpointError::InvalidQuerySourceType { .. } => ErrorCode::InvalidQueryType,
        EndpointError::InvalidEventSinkType { .. } => ErrorCode::InvalidEventType,
    };
    let error_message = error.to_string();

    to_error(error_code, error_message)
}

/// Attempts a cast from a `MonotonicTime` to a protobuf `Timestamp`.
///
/// This will fail if the time is outside the protobuf-specified range for
/// timestamps (0001-01-01 00:00:00 to 9999-12-31 23:59:59).
pub(crate) fn monotonic_to_timestamp(monotonic_time: MonotonicTime) -> Option<Timestamp> {
    // Unix timestamp for 0001-01-01 00:00:00, the minimum accepted by
    // protobuf's specification for the `Timestamp` type.
    const MIN_SECS: i64 = -62135596800;
    // Unix timestamp for 9999-12-31 23:59:59, the maximum accepted by
    // protobuf's specification for the `Timestamp` type.
    const MAX_SECS: i64 = 253402300799;

    let secs = monotonic_time.as_secs();
    if !(MIN_SECS..=MAX_SECS).contains(&secs) {
        return None;
    }

    Some(Timestamp {
        seconds: secs,
        nanos: monotonic_time.subsec_nanos() as i32,
    })
}

/// Attempts a cast from a protobuf `Timestamp` to a `MonotonicTime`.
///
/// This should never fail provided that the `Timestamp` complies with the
/// protobuf specification. It can only fail if the nanosecond part is negative
/// or greater than 999'999'999.
pub(crate) fn timestamp_to_monotonic(timestamp: Timestamp) -> Option<MonotonicTime> {
    let nanos: u32 = timestamp.nanos.try_into().ok()?;

    MonotonicTime::new(timestamp.seconds, nanos)
}

/// Attempts a cast from a protobuf `Duration` to a `std::time::Duration`.
///
/// If the `Duration` complies with the protobuf specification, this can only
/// fail if the duration is negative.
pub(crate) fn to_positive_duration(duration: prost_types::Duration) -> Option<Duration> {
    if duration.seconds < 0 || duration.nanos < 0 {
        return None;
    }

    Some(Duration::new(
        duration.seconds as u64,
        duration.nanos as u32,
    ))
}

/// Attempts a cast from a protobuf `Duration` to a strictly positive
/// `std::time::Duration`.
///
/// If the `Duration` complies with the protobuf specification, this can only
/// fail if the duration is negative or null.
pub(crate) fn to_strictly_positive_duration(duration: prost_types::Duration) -> Option<Duration> {
    if duration.seconds < 0 || duration.nanos < 0 || (duration.seconds == 0 && duration.nanos == 0)
    {
        return None;
    }

    Some(Duration::new(
        duration.seconds as u64,
        duration.nanos as u32,
    ))
}

#[tonic::async_trait]
impl simulation_server::Simulation for GrpcSimulationService {
    //-----------
    // Terminate.
    //-----------

    async fn terminate(
        &self,
        _request: Request<TerminateRequest>,
    ) -> Result<Response<TerminateReply>, Status> {
        *self.controller_service.lock().unwrap() = ControllerService::Halted;
        *self.inspector_service.write().unwrap() = InspectorService::Halted;
        *self.monitor_service.lock().unwrap() = MonitorService::Halted;
        *self.scheduler_service.lock().unwrap() = SchedulerService::Halted;

        Ok(Response::new(TerminateReply {
            result: Some(terminate_reply::Result::Empty(())),
        }))
    }

    //--------------
    // Build service.
    //--------------

    async fn build(&self, request: Request<BuildRequest>) -> Result<Response<BuildReply>, Status> {
        let request = request.into_inner();

        let reply = self.build_service.lock().unwrap().build(request);

        Ok(Response::new(BuildReply {
            result: Some(match reply {
                Ok(()) => build_reply::Result::Empty(()),
                Err(e) => build_reply::Result::Error(e),
            }),
        }))
    }

    //--------------
    // Init service.
    //--------------

    async fn init(&self, request: Request<InitRequest>) -> Result<Response<InitReply>, Status> {
        let request = request.into_inner();

        let reply =
            self.build_service
                .lock()
                .unwrap()
                .init(request)
                .map(|(simulation, endpoints)| {
                    self.start_services(simulation, endpoints);
                });

        Ok(Response::new(InitReply {
            result: Some(match reply {
                Ok(()) => init_reply::Result::Empty(()),
                Err(e) => init_reply::Result::Error(e),
            }),
        }))
    }
    async fn init_and_run(
        &self,
        request: Request<InitAndRunRequest>,
    ) -> Result<Response<InitAndRunReply>, Status> {
        // This is a composite request that is splitted into 2 sub-requests.
        let request = request.into_inner();
        let init_request = InitRequest { time: request.time };

        let reply =
            self.build_service
                .lock()
                .unwrap()
                .init(init_request)
                .map(|(simulation, endpoints)| {
                    self.start_services(simulation, endpoints);
                });

        // Important: release the lock on the build service before calling
        // `run``, as `run` is blocking.
        let reply = match reply {
            Ok(()) => {
                self.execute_controller_fn(RunRequest {}, ControllerService::run)
                    .await
            }
            Err(e) => Err(e),
        };

        Ok(Response::new(InitAndRunReply {
            result: Some(match reply {
                Ok(timestamp) => init_and_run_reply::Result::Time(timestamp),
                Err(e) => init_and_run_reply::Result::Error(e),
            }),
        }))
    }
    async fn restore(
        &self,
        request: Request<RestoreRequest>,
    ) -> Result<Response<RestoreReply>, Status> {
        let request = request.into_inner();

        let reply =
            self.build_service
                .lock()
                .unwrap()
                .restore(request)
                .map(|(simulation, endpoints)| {
                    self.start_services(simulation, endpoints);
                });

        Ok(Response::new(RestoreReply {
            result: Some(match reply {
                Ok(()) => restore_reply::Result::Empty(()),
                Err(e) => restore_reply::Result::Error(e),
            }),
        }))
    }
    async fn restore_and_run(
        &self,
        request: Request<RestoreAndRunRequest>,
    ) -> Result<Response<RestoreAndRunReply>, Status> {
        // This is a composite request that is splitted into 2 sub-requests.
        let request = request.into_inner();
        let restore_request = RestoreRequest {
            state: request.state,
        };

        let reply = self
            .build_service
            .lock()
            .unwrap()
            .restore(restore_request)
            .map(|(simulation, endpoints)| {
                self.start_services(simulation, endpoints);
            });

        // Important: release the lock on the build service before calling
        // `run``, as `run` is blocking.
        let reply = match reply {
            Ok(()) => {
                self.execute_controller_fn(RunRequest {}, ControllerService::run)
                    .await
            }
            Err(e) => Err(e),
        };

        Ok(Response::new(RestoreAndRunReply {
            result: Some(match reply {
                Ok(timestamp) => restore_and_run_reply::Result::Time(timestamp),
                Err(e) => restore_and_run_reply::Result::Error(e),
            }),
        }))
    }

    //--------------------
    // Controller service.
    //--------------------

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
    async fn run(&self, request: Request<RunRequest>) -> Result<Response<RunReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .execute_controller_fn(request, ControllerService::run)
            .await;

        Ok(Response::new(RunReply {
            result: Some(match reply {
                Ok(timestamp) => run_reply::Result::Time(timestamp),
                Err(error) => run_reply::Result::Error(error),
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

    //-------------------
    // Scheduler service.
    //-------------------

    async fn time(&self, request: Request<TimeRequest>) -> Result<Response<TimeReply>, Status> {
        let request = request.into_inner();

        Ok(Response::new(TimeReply {
            result: Some(match self.scheduler_service.lock().unwrap().time(request) {
                Ok(timestamp) => time_reply::Result::Time(timestamp),
                Err(e) => time_reply::Result::Error(e),
            }),
        }))
    }
    async fn halt(&self, request: Request<HaltRequest>) -> Result<Response<HaltReply>, Status> {
        let request = request.into_inner();

        Ok(Response::new(HaltReply {
            result: Some(match self.scheduler_service.lock().unwrap().halt(request) {
                Ok(()) => halt_reply::Result::Empty(()),
                Err(e) => halt_reply::Result::Error(e),
            }),
        }))
    }
    async fn schedule_event(
        &self,
        request: Request<ScheduleEventRequest>,
    ) -> Result<Response<ScheduleEventReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .scheduler_service
            .lock()
            .unwrap()
            .schedule_event(request);

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
        let reply = self.scheduler_service.lock().unwrap().cancel_event(request);

        Ok(Response::new(CancelEventReply {
            result: Some(match reply {
                Ok(()) => cancel_event_reply::Result::Empty(()),
                Err(error) => cancel_event_reply::Result::Error(error),
            }),
        }))
    }

    //-------------------
    // Inspector service.
    //-------------------

    async fn list_event_sources(
        &self,
        request: Request<ListEventSourcesRequest>,
    ) -> Result<Response<ListEventSourcesReply>, Status> {
        let request = request.into_inner();
        let reply = self
            .inspector_service
            .read()
            .unwrap()
            .list_event_sources(request);

        Ok(Response::new(match reply {
            Ok(sources) => ListEventSourcesReply {
                sources,
                result: Some(list_event_sources_reply::Result::Empty(())),
            },
            Err(e) => ListEventSourcesReply {
                sources: Vec::new(),
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
            .unwrap()
            .get_event_source_schemas(request);

        Ok(Response::new(match reply {
            Ok(schemas) => GetEventSourceSchemasReply {
                schemas,
                result: Some(get_event_source_schemas_reply::Result::Empty(())),
            },
            Err(e) => GetEventSourceSchemasReply {
                schemas: Vec::new(),
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
            .unwrap()
            .list_query_sources(request);

        Ok(Response::new(match reply {
            Ok(sources) => ListQuerySourcesReply {
                sources,
                result: Some(list_query_sources_reply::Result::Empty(())),
            },
            Err(e) => ListQuerySourcesReply {
                sources: Vec::new(),
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
            .unwrap()
            .get_query_source_schemas(request);

        Ok(Response::new(match reply {
            Ok(schemas) => GetQuerySourceSchemasReply {
                schemas,
                result: Some(get_query_source_schemas_reply::Result::Empty(())),
            },
            Err(e) => GetQuerySourceSchemasReply {
                schemas: Vec::new(),
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
            .unwrap()
            .list_event_sinks(request);

        Ok(Response::new(match reply {
            Ok(sinks) => ListEventSinksReply {
                sinks,
                result: Some(list_event_sinks_reply::Result::Empty(())),
            },
            Err(e) => ListEventSinksReply {
                sinks: Vec::new(),
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
            .unwrap()
            .get_event_sink_schemas(request);

        Ok(Response::new(match reply {
            Ok(schemas) => GetEventSinkSchemasReply {
                schemas,
                result: Some(get_event_sink_schemas_reply::Result::Empty(())),
            },
            Err(e) => GetEventSinkSchemasReply {
                schemas: Vec::new(),
                result: Some(get_event_sink_schemas_reply::Result::Error(e)),
            },
        }))
    }

    //-----------------
    // Monitor service.
    //-----------------

    async fn try_read_events(
        &self,
        request: Request<TryReadEventsRequest>,
    ) -> Result<Response<TryReadEventsReply>, Status> {
        let request = request.into_inner();

        let reply = self
            .monitor_service
            .lock()
            .unwrap()
            .try_read_events(request);

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
        let reply = self.monitor_service.lock().unwrap().enable_sink(request);

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
        let reply = self.monitor_service.lock().unwrap().disable_sink(request);

        Ok(Response::new(DisableSinkReply {
            result: Some(match reply {
                Ok(()) => disable_sink_reply::Result::Empty(()),
                Err(e) => disable_sink_reply::Result::Error(e),
            }),
        }))
    }
}
