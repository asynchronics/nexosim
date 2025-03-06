use std::fmt;
use std::time::Duration;

use crate::registry::EventSinkRegistry;

use super::super::codegen::simulation::*;
use super::{simulation_not_started_error, to_error, to_positive_duration};

/// Protobuf-based simulation monitor.
///
/// A `MonitorService` enables the monitoring of the event sinks of a
/// [`Simulation`](crate::simulation::Simulation).
pub(crate) enum MonitorService {
    Started {
        event_sink_registry: EventSinkRegistry,
    },
    NotStarted,
}

impl MonitorService {
    /// Reads all events from an event sink.
    pub(crate) fn read_events(&self, request: ReadEventsRequest) -> ReadEventsReply {
        let reply = match self {
            Self::Started {
                event_sink_registry,
            } => move || -> Result<Vec<Vec<u8>>, Error> {
                let sink_name = &request.sink_name;

                let mut sink = event_sink_registry.get(sink_name).ok_or(to_error(
                    ErrorCode::SinkNotFound,
                    format!("no sink is registered with the name '{}'", sink_name),
                ))?;

                sink.collect().map_err(|e| {
                    to_error(
                        ErrorCode::InvalidMessage,
                        format!(
                            "the event could not be serialized from type '{}': {}",
                            sink.event_type_name(),
                            e
                        ),
                    )
                })
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        match reply {
            Ok(events) => ReadEventsReply {
                events,
                result: Some(read_events_reply::Result::Empty(())),
            },
            Err(error) => ReadEventsReply {
                events: Vec::new(),
                result: Some(read_events_reply::Result::Error(error)),
            },
        }
    }

    /// Waits for an event from an event sink and reads it.
    pub(crate) fn await_event(&self, request: AwaitEventRequest) -> AwaitEventReply {
        let reply = match self {
            Self::Started {
                event_sink_registry,
            } => move || -> Result<Vec<u8>, Error> {
                let sink_name = &request.sink_name;

                let mut sink = event_sink_registry.get(sink_name).ok_or(to_error(
                    ErrorCode::SinkNotFound,
                    format!("no sink is registered with the name '{}'", sink_name),
                ))?;

                let timeout = request.timeout.map_or(Ok(Duration::ZERO), |timeout| {
                    to_positive_duration(timeout).ok_or(to_error(
                        ErrorCode::InvalidTimeout,
                        "the specified timeout is negative",
                    ))
                })?;
                sink.await_event(timeout).map_err(|e| {
                    to_error(
                        ErrorCode::InvalidMessage,
                        format!(
                            "the event could not be serialized from type '{}': {}",
                            sink.event_type_name(),
                            e
                        ),
                    )
                })
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        match reply {
            Ok(event) => AwaitEventReply {
                result: Some(await_event_reply::Result::Event(event)),
            },
            Err(error) => AwaitEventReply {
                result: Some(await_event_reply::Result::Error(error)),
            },
        }
    }

    /// Opens an event sink.
    pub(crate) fn open_sink(&mut self, request: OpenSinkRequest) -> OpenSinkReply {
        let reply = match self {
            Self::Started {
                event_sink_registry,
            } => {
                let sink_name = &request.sink_name;

                if let Some(sink) = event_sink_registry.get_mut(sink_name) {
                    sink.open();

                    open_sink_reply::Result::Empty(())
                } else {
                    open_sink_reply::Result::Error(to_error(
                        ErrorCode::SinkNotFound,
                        format!("no sink is registered with the name '{}'", sink_name),
                    ))
                }
            }
            Self::NotStarted => open_sink_reply::Result::Error(simulation_not_started_error()),
        };

        OpenSinkReply {
            result: Some(reply),
        }
    }

    /// Closes an event sink.
    pub(crate) fn close_sink(&mut self, request: CloseSinkRequest) -> CloseSinkReply {
        let reply = match self {
            Self::Started {
                event_sink_registry,
            } => {
                let sink_name = &request.sink_name;

                if let Some(sink) = event_sink_registry.get_mut(sink_name) {
                    sink.close();

                    close_sink_reply::Result::Empty(())
                } else {
                    close_sink_reply::Result::Error(to_error(
                        ErrorCode::SinkNotFound,
                        format!("no sink is registered with the name '{}'", sink_name),
                    ))
                }
            }
            Self::NotStarted => close_sink_reply::Result::Error(simulation_not_started_error()),
        };

        CloseSinkReply {
            result: Some(reply),
        }
    }
}

impl fmt::Debug for MonitorService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimulationService").finish_non_exhaustive()
    }
}
