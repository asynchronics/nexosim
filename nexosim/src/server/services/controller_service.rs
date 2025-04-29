use std::fmt;
use std::sync::Arc;

use prost_types::Timestamp;

use crate::registry::{EventSourceRegistry, QuerySourceRegistry};
use crate::simulation::Simulation;
use crate::util::serialization::get_serialization_config;

use super::super::codegen::simulation::*;
use super::{
    map_execution_error, monotonic_to_timestamp, simulation_not_started_error,
    timestamp_to_monotonic, to_error, to_positive_duration,
};

/// Protobuf-based simulation controller.
///
/// A `ControllerService` controls the execution of the simulation. Note that
/// all its methods block until execution completes.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ControllerService {
    NotStarted,
    Started {
        cfg: Vec<u8>,
        simulation: Simulation,
        event_source_registry: Arc<EventSourceRegistry>,
        query_source_registry: QuerySourceRegistry,
    },
}

impl ControllerService {
    /// Advances simulation time to that of the next scheduled event, processing
    /// that event as well as all other events scheduled for the same time.
    ///
    /// Processing is gated by a (possibly blocking) call to
    /// [`Clock::synchronize`](crate::time::Clock::synchronize) on the
    /// configured simulation clock. This method blocks until all newly
    /// processed events have completed.
    pub(crate) fn step(&mut self, _request: StepRequest) -> StepReply {
        let reply = match self {
            Self::Started { simulation, .. } => match simulation.step() {
                Ok(()) => {
                    if let Some(timestamp) = monotonic_to_timestamp(simulation.time()) {
                        step_reply::Result::Time(timestamp)
                    } else {
                        step_reply::Result::Error(to_error(
                            ErrorCode::SimulationTimeOutOfRange,
                            "the final simulation time is out of range",
                        ))
                    }
                }
                Err(e) => step_reply::Result::Error(map_execution_error(e)),
            },
            Self::NotStarted => step_reply::Result::Error(simulation_not_started_error()),
        };

        StepReply {
            result: Some(reply),
        }
    }

    /// Iteratively advances the simulation time until the specified deadline,
    /// as if by calling
    /// [`Simulation::step`](crate::simulation::Simulation::step) repeatedly.
    ///
    /// This method blocks until all events scheduled up to the specified target
    /// time have completed. The simulation time upon completion is equal to the
    /// specified target time, whether or not an event was scheduled for that
    /// time.
    pub(crate) fn step_until(&mut self, request: StepUntilRequest) -> StepUntilReply {
        let reply = match self {
            Self::Started { simulation, .. } => move || -> Result<Timestamp, Error> {
                let deadline = request.deadline.ok_or(to_error(
                    ErrorCode::MissingArgument,
                    "missing deadline argument",
                ))?;

                match deadline {
                    step_until_request::Deadline::Time(time) => {
                        let time = timestamp_to_monotonic(time).ok_or(to_error(
                            ErrorCode::InvalidTime,
                            "out-of-range nanosecond field",
                        ))?;

                        simulation.step_until(time).map_err(map_execution_error)?;
                    }
                    step_until_request::Deadline::Duration(duration) => {
                        let duration = to_positive_duration(duration).ok_or(to_error(
                            ErrorCode::InvalidDeadline,
                            "the specified deadline lies in the past",
                        ))?;

                        simulation
                            .step_until(duration)
                            .map_err(map_execution_error)?;
                    }
                };

                let timestamp = monotonic_to_timestamp(simulation.time()).ok_or(to_error(
                    ErrorCode::SimulationTimeOutOfRange,
                    "the final simulation time is out of range",
                ))?;

                Ok(timestamp)
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        StepUntilReply {
            result: Some(match reply {
                Ok(timestamp) => step_until_reply::Result::Time(timestamp),
                Err(error) => step_until_reply::Result::Error(error),
            }),
        }
    }

    /// Iteratively advances the simulation time, as if by calling
    /// [`Simulation::step`] repeatedly.
    ///
    /// This method blocks until the simulation is halted or all scheduled
    /// events have completed.
    pub(crate) fn step_unbounded(&mut self, _request: StepUnboundedRequest) -> StepUnboundedReply {
        let reply = match self {
            Self::Started { simulation, .. } => move || -> Result<Timestamp, Error> {
                simulation.step_unbounded().map_err(map_execution_error)?;

                let timestamp = monotonic_to_timestamp(simulation.time()).ok_or(to_error(
                    ErrorCode::SimulationTimeOutOfRange,
                    "the final simulation time is out of range",
                ))?;

                Ok(timestamp)
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        StepUnboundedReply {
            result: Some(match reply {
                Ok(timestamp) => step_unbounded_reply::Result::Time(timestamp),
                Err(error) => step_unbounded_reply::Result::Error(error),
            }),
        }
    }

    /// Broadcasts an event from an event source immediately, blocking until
    /// completion.
    ///
    /// Simulation time remains unchanged.
    pub(crate) fn process_event(&mut self, request: ProcessEventRequest) -> ProcessEventReply {
        let reply = match self {
            Self::Started {
                simulation,
                event_source_registry,
                ..
            } => move || -> Result<(), Error> {
                let source_name = &request.source_name;
                let event = &request.event;

                let source = event_source_registry.get(source_name).ok_or(to_error(
                    ErrorCode::SourceNotFound,
                    "no source is registered with the name '{}'".to_string(),
                ))?;

                // let event = source.event(event).map_err(|e| {
                //     to_error(
                //         ErrorCode::InvalidMessage,
                //         format!(
                //             "the event could not be deserialized as type '{}': {}",
                //             source.event_type_name(),
                //             e
                //         ),
                //     )
                // })?;
                let event = source.into_future(event).map_err(|e| {
                    to_error(
                        ErrorCode::InvalidMessage,
                        format!(
                            "the event could not be deserialized as type '{}': {}",
                            source.event_type_name(),
                            e
                        ),
                    )
                })?;

                simulation.process(event).map_err(map_execution_error)
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        ProcessEventReply {
            result: Some(match reply {
                Ok(()) => process_event_reply::Result::Empty(()),
                Err(error) => process_event_reply::Result::Error(error),
            }),
        }
    }

    /// Broadcasts a query from a query source immediately, blocking until
    /// completion.
    ///
    /// Simulation time remains unchanged.
    pub(crate) fn process_query(&mut self, request: ProcessQueryRequest) -> ProcessQueryReply {
        let reply = match self {
            Self::Started {
                simulation,
                query_source_registry,
                ..
            } => move || -> Result<Vec<Vec<u8>>, Error> {
                let source_name = &request.source_name;
                let request = &request.request;

                let source = query_source_registry.get(source_name).ok_or(to_error(
                    ErrorCode::SourceNotFound,
                    "no source is registered with the name '{}'".to_string(),
                ))?;

                let (query, mut promise) = source.into_future(request).map_err(|e| {
                    to_error(
                        ErrorCode::InvalidMessage,
                        format!(
                            "the request could not be deserialized as type '{}': {}",
                            source.request_type_name(),
                            e
                        ),
                    )
                })?;

                simulation.process(query).map_err(map_execution_error)?;

                let replies = promise.take_collect().ok_or(to_error(
                    ErrorCode::SimulationBadQuery,
                    "a reply to the query was expected but none was available; maybe the target model was not added to the simulation?".to_string(),
                ))?;

                replies.map_err(|e| {
                    to_error(
                        ErrorCode::InvalidMessage,
                        format!(
                            "the reply could not be serialized as type '{}': {}",
                            source.reply_type_name(),
                            e
                        ),
                    )
                })
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        match reply {
            Ok(replies) => ProcessQueryReply {
                replies,
                result: Some(process_query_reply::Result::Empty(())),
            },
            Err(error) => ProcessQueryReply {
                replies: Vec::new(),
                result: Some(process_query_reply::Result::Error(error)),
            },
        }
    }

    pub(crate) fn save(&mut self, _request: SaveRequest) -> SaveReply {
        match self {
            Self::NotStarted => SaveReply {
                result: Some(save_reply::Result::Error(simulation_not_started_error())),
            },
            Self::Started {
                cfg, simulation, ..
            } => {
                let state = simulation.serialize_state();
                SaveReply {
                    result: Some(save_reply::Result::State(
                        bincode::serde::encode_to_vec((cfg, state), get_serialization_config())
                            .unwrap(),
                    )),
                }
            }
        }
    }
}

impl fmt::Debug for ControllerService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControllerService").finish_non_exhaustive()
    }
}
