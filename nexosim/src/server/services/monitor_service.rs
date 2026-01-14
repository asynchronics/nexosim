use std::fmt;

use futures_util::StreamExt;
use tokio::sync::Mutex as TokioMutex;
use tokio::time as tokio_time;

use crate::endpoints::EventSinkRegistry;
use crate::path::Path as NexosimPath;

use super::super::codegen::simulation::*;
use super::{simulation_halted_error, to_error, to_positive_duration};

/// Protobuf-based simulation monitor.
///
/// A `MonitorService` enables the monitoring of the event sinks of a
/// [`Simulation`](crate::simulation::Simulation).
pub(crate) enum MonitorService {
    Halted,
    Started {
        event_sink_registry: EventSinkRegistry,
    },
}

impl MonitorService {
    /// Reads all events from an event sink.
    pub(crate) fn try_read_events(
        &mut self,
        request: TryReadEventsRequest,
    ) -> Result<Vec<Vec<u8>>, Error> {
        match self {
            Self::Started {
                event_sink_registry,
            } => {
                let sink_path: &NexosimPath = &request
                    .sink
                    .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing event sink path"))?
                    .segments
                    .into();

                let sink = match event_sink_registry.get_entry_mut(sink_path) {
                    Ok(sink) => sink,
                    Err(_) => {
                        return Err(if event_sink_registry.has_sink(sink_path) {
                            to_error(
                                ErrorCode::SinkReadRace,
                                format!(
                                    "attempting concurrent read operation on sink '{sink_path}'"
                                ),
                            )
                        } else {
                            sink_not_found_error(sink_path)
                        });
                    }
                };

                let mut encoded_events = Vec::new();
                while let Some(encoded_event) = sink.try_read() {
                    match encoded_event {
                        Ok(encoded_event) => encoded_events.push(encoded_event),
                        Err(e) => {
                            return Err(to_error(
                                ErrorCode::InvalidMessage,
                                format!(
                                    "the event from sink '{sink_path}' could not be serialized from type '{}': {e}",
                                    sink.event_type_name(),
                                ),
                            ));
                        }
                    }
                }

                Ok(encoded_events)
            }
            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Enables an event sink.
    pub(crate) fn enable_sink(&mut self, request: EnableSinkRequest) -> Result<(), Error> {
        match self {
            Self::Started {
                event_sink_registry,
            } => {
                let sink_path: &NexosimPath = &request
                    .sink
                    .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing event sink path"))?
                    .segments
                    .into();

                if let Ok(sink) = event_sink_registry.get_entry_mut(sink_path) {
                    sink.enable();

                    Ok(())
                } else {
                    Err(if event_sink_registry.has_sink(sink_path) {
                        to_error(
                            ErrorCode::SinkReadRace,
                            format!(
                                "attempting to enable sink '{sink_path}' while a read operation is ongoing"
                            ),
                        )
                    } else {
                        sink_not_found_error(sink_path)
                    })
                }
            }
            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Disables an event sink.
    pub(crate) fn disable_sink(&mut self, request: DisableSinkRequest) -> Result<(), Error> {
        match self {
            Self::Started {
                event_sink_registry,
            } => {
                let sink_path: &NexosimPath = &request
                    .sink
                    .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing event sink path"))?
                    .segments
                    .into();

                if let Ok(sink) = event_sink_registry.get_entry_mut(sink_path) {
                    sink.disable();

                    Ok(())
                } else {
                    Err(if event_sink_registry.has_sink(sink_path) {
                        to_error(
                            ErrorCode::SinkReadRace,
                            format!(
                                "attempting to disable sink '{sink_path}' while a read operation is ongoing"
                            ),
                        )
                    } else {
                        sink_not_found_error(sink_path)
                    })
                }
            }
            Self::Halted => Err(simulation_halted_error()),
        }
    }
}

impl fmt::Debug for MonitorService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimulationService").finish_non_exhaustive()
    }
}

/// Waits for an event from an event sink.
///
/// To avoid blocking the thread while waiting for an event, this is implemented
/// as an async function. It cannot be a `MonitorService` method because it
/// needs `MonitorService` to be under a mutex so it can be locked twice: once
/// to rent the sink and once to return it.
pub(crate) async fn monitor_service_read_event(
    service: &TokioMutex<MonitorService>,
    request: ReadEventRequest,
) -> Result<Vec<u8>, Error> {
    let sink_path: &NexosimPath = &request
        .sink
        .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing event sink path"))?
        .segments
        .into();

    // Very important: the lock is released immediately after renting the
    // sink so we do not block concurrent `MonitorService` requests.
    let mut sink = match &mut *service.lock().await {
        MonitorService::Started {
            event_sink_registry,
        } => {
            // Rent the sink.
            match event_sink_registry.rent_entry(sink_path) {
                Ok(sink) => sink,
                Err(_) => {
                    return Err(if event_sink_registry.has_sink(sink_path) {
                        to_error(
                            ErrorCode::SinkReadRace,
                            format!("attempting concurrent read operation on sink '{sink_path}'"),
                        )
                    } else {
                        sink_not_found_error(sink_path)
                    });
                }
            }
        }
        MonitorService::Halted => return Err(simulation_halted_error()),
    };

    // Await the event, possibly applying a timeout.
    let event = match request.timeout {
        Some(timeout) => {
            let timeout = to_positive_duration(timeout).ok_or_else(|| {
                to_error(
                    ErrorCode::InvalidTimeout,
                    "the specified timeout is negative",
                )
            })?;

            tokio_time::timeout(timeout, sink.next())
                .await
                .map_err(|_| {
                    to_error(
                        ErrorCode::SinkReadTimeout,
                        format!("the read operation on sink '{sink_path}' timed out",),
                    )
                })?
        }
        None => sink.next().await,
    };

    let reply = event
        .ok_or_else(|| {
            to_error(
                ErrorCode::SinkTerminated,
                format!("sink '{sink_path}' has not sender"),
            )
        })
        .and_then(|s| {
            s.map_err(|e|
            to_error(
                ErrorCode::InvalidMessage,
                format!(
                    "the event from sink '{sink_path}' could not be serialized from type '{}': {e}",
                    sink.event_type_name(),
                ),
            ))
        });

    // Return the sink to the registry
    match &mut *service.lock().await {
        MonitorService::Started {
            event_sink_registry,
        } => {
            event_sink_registry.return_entry(sink_path, sink).unwrap(); // always succeed: the sink name is registered
        }
        MonitorService::Halted => return Err(simulation_halted_error()),
    };

    reply
}

/// An error returned when a the simulation time is out of the range supported
/// by gRPC.
fn sink_not_found_error(sink: &NexosimPath) -> Error {
    to_error(
        ErrorCode::SinkNotFound,
        format!("no sink is registered with the name '{sink}'"),
    )
}
