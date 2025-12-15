use std::fmt;

use futures_util::StreamExt;
use tokio::sync::Mutex as TokioMutex;
use tokio::time as tokio_time;

use crate::endpoints::EventSinkRegistry;

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
                let sink_name = &request.sink_name;

                let sink = match event_sink_registry.get_entry_mut(sink_name) {
                    Ok(sink) => sink,
                    Err(_) => {
                        return Err(if event_sink_registry.has_sink(sink_name) {
                            to_error(
                                ErrorCode::SinkReadRace,
                                format!(
                                    "attempting concurrent read operation on sink '{sink_name}'"
                                ),
                            )
                        } else {
                            sink_not_found_error(sink_name)
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
                                    "the event from sink '{sink_name}' could not be serialized from type '{}': {e}",
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

    /// Opens an event sink.
    pub(crate) fn open_sink(&mut self, request: OpenSinkRequest) -> Result<(), Error> {
        match self {
            Self::Started {
                event_sink_registry,
            } => {
                let sink_name = &request.sink_name;

                if let Ok(sink) = event_sink_registry.get_entry_mut(sink_name) {
                    sink.open();

                    Ok(())
                } else {
                    Err(if event_sink_registry.has_sink(sink_name) {
                        to_error(
                            ErrorCode::SinkReadRace,
                            format!(
                                "attempting to open sink '{sink_name}' while a read operation is ongoing"
                            ),
                        )
                    } else {
                        sink_not_found_error(sink_name)
                    })
                }
            }
            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Closes an event sink.
    pub(crate) fn close_sink(&mut self, request: CloseSinkRequest) -> Result<(), Error> {
        match self {
            Self::Started {
                event_sink_registry,
            } => {
                let sink_name = &request.sink_name;

                if let Ok(sink) = event_sink_registry.get_entry_mut(sink_name) {
                    sink.close();

                    Ok(())
                } else {
                    Err(if event_sink_registry.has_sink(sink_name) {
                        to_error(
                            ErrorCode::SinkReadRace,
                            format!(
                                "attempting to close sink '{sink_name}' while a read operation is ongoing"
                            ),
                        )
                    } else {
                        sink_not_found_error(sink_name)
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
    let sink_name = &request.sink_name;

    // Very important: the lock is released immediately after renting the
    // sink so we do not block concurrent `MonitorService` requests.
    let mut sink = match &mut *service.lock().await {
        MonitorService::Started {
            event_sink_registry,
        } => {
            // Rent the sink.
            match event_sink_registry.rent_entry(sink_name) {
                Ok(sink) => sink,
                Err(_) => {
                    return Err(if event_sink_registry.has_sink(sink_name) {
                        to_error(
                            ErrorCode::SinkReadRace,
                            format!("attempting concurrent read operation on sink '{sink_name}'"),
                        )
                    } else {
                        sink_not_found_error(sink_name)
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
                        format!("the read operation on sink '{sink_name}' timed out",),
                    )
                })?
        }
        None => sink.next().await,
    };

    let reply = event
        .ok_or_else(|| {
            to_error(
                ErrorCode::SinkTerminated,
                format!("sink '{sink_name}' has not sender"),
            )
        })
        .and_then(|s| {
            s.map_err(|e|
            to_error(
                ErrorCode::InvalidMessage,
                format!(
                    "the event from sink '{sink_name}' could not be serialized from type '{}': {e}",
                    sink.event_type_name(),
                ),
            ))
        });

    // Return the sink to the registry
    match &mut *service.lock().await {
        MonitorService::Started {
            event_sink_registry,
        } => {
            event_sink_registry.return_entry(sink_name, sink).unwrap(); // always succeed: the sink name is registered
        }
        MonitorService::Halted => return Err(simulation_halted_error()),
    };

    reply
}

/// An error returned when a the simulation time is out of the range supported
/// by gRPC.
fn sink_not_found_error(sink_name: &str) -> Error {
    to_error(
        ErrorCode::SinkNotFound,
        format!("no sink is registered with the name '{sink_name}'"),
    )
}
