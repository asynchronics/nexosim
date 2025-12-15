use std::fmt;
use std::sync::Arc;

use prost_types::Timestamp;

use crate::endpoints::{EventSourceRegistry, QuerySourceRegistry};
use crate::server::services::map_endpoint_error;
use crate::simulation::Simulation;

use super::super::codegen::simulation::*;
use super::{
    map_execution_error, monotonic_to_timestamp, simulation_halted_error, timestamp_to_monotonic,
    to_error, to_positive_duration,
};

/// Protobuf-based simulation controller.
///
/// A `ControllerService` controls the execution of the simulation. Note that
/// all its methods block until execution completes.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ControllerService {
    Halted,
    Started {
        cfg: Vec<u8>,
        simulation: Simulation,
        event_source_registry: Arc<EventSourceRegistry>,
        query_source_registry: Arc<QuerySourceRegistry>,
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
    pub(crate) fn step(&mut self, _request: StepRequest) -> Result<Timestamp, Error> {
        let Self::Started { simulation, .. } = self else {
            return Err(simulation_halted_error());
        };

        simulation
            .step()
            .map_err(map_execution_error)
            .and_then(|()| {
                monotonic_to_timestamp(simulation.time()).ok_or_else(final_simulation_time_error)
            })
    }

    /// Iteratively advances the simulation time until the specified deadline,
    /// as if by calling
    /// [`Simulation::step`](crate::simulation::Simulation::step) repeatedly.
    ///
    /// This method blocks until all events scheduled up to the specified target
    /// time have completed. The simulation time upon completion is equal to the
    /// specified target time, whether or not an event was scheduled for that
    /// time.
    pub(crate) fn step_until(&mut self, request: StepUntilRequest) -> Result<Timestamp, Error> {
        let Self::Started { simulation, .. } = self else {
            return Err(simulation_halted_error());
        };

        let deadline = request
            .deadline
            .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing deadline argument"))?;

        match deadline {
            step_until_request::Deadline::Time(time) => {
                let time = timestamp_to_monotonic(time).ok_or_else(|| {
                    to_error(ErrorCode::InvalidTime, "out-of-range nanosecond field")
                })?;

                simulation.step_until(time).map_err(map_execution_error)?;
            }
            step_until_request::Deadline::Duration(duration) => {
                let duration = to_positive_duration(duration).ok_or_else(|| {
                    to_error(
                        ErrorCode::InvalidDeadline,
                        "the specified deadline lies in the past",
                    )
                })?;

                simulation
                    .step_until(duration)
                    .map_err(map_execution_error)?;
            }
        };

        monotonic_to_timestamp(simulation.time()).ok_or_else(final_simulation_time_error)
    }

    /// Iteratively advances the simulation time, as if by calling
    /// [`Simulation::step`] repeatedly.
    ///
    /// This method blocks until the simulation is halted or all scheduled
    /// events have completed.
    pub(crate) fn step_unbounded(
        &mut self,
        _request: StepUnboundedRequest,
    ) -> Result<Timestamp, Error> {
        let Self::Started { simulation, .. } = self else {
            return Err(simulation_halted_error());
        };

        simulation.step_unbounded().map_err(map_execution_error)?;

        monotonic_to_timestamp(simulation.time()).ok_or_else(final_simulation_time_error)
    }

    /// Broadcasts an event from an event source immediately, blocking until
    /// completion.
    ///
    /// Simulation time remains unchanged.
    pub(crate) fn process_event(&mut self, request: ProcessEventRequest) -> Result<(), Error> {
        let Self::Started {
            simulation,
            event_source_registry,
            ..
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let source_name = &request.source_name;
        let event = &request.event;

        let source = event_source_registry
            .get(source_name)
            .map_err(map_endpoint_error)?;

        let event_id = source.get_event_id();
        let arg = source.deserialize_arg(event).map_err(|e| {
            to_error(
                ErrorCode::InvalidMessage,
                format!(
                    "the event could not be deserialized as type '{}': {}",
                    source.event_type_name(),
                    e
                ),
            )
        })?;

        simulation
            .process_event_erased(&event_id, arg)
            .map_err(map_execution_error)
    }

    /// Broadcasts a query from a query source immediately, blocking until
    /// completion.
    ///
    /// Simulation time remains unchanged.
    pub(crate) fn process_query(
        &mut self,
        request: ProcessQueryRequest,
    ) -> Result<Vec<Vec<u8>>, Error> {
        let Self::Started {
            simulation,
            query_source_registry,
            ..
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let source_name = &request.source_name;
        let request = &request.request;

        let source = query_source_registry
            .get(source_name)
            .map_err(map_endpoint_error)?;

        let query_id = source.get_query_id();
        let arg = source.deserialize_arg(request).map_err(|e| {
            to_error(
                ErrorCode::InvalidMessage,
                format!(
                    "the request could not be deserialized as type '{}': {}",
                    source.request_type_name(),
                    e
                ),
            )
        })?;
        let (tx, mut rx) = source.replier();

        simulation
            .process_query_erased(&query_id, arg, tx)
            .map_err(map_execution_error)?;

        let replies = rx.take_collect().ok_or_else(|| to_error(
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
    }

    /// Saves and returns current simulation state in a serialized form.
    pub(crate) fn save(&mut self, _: SaveRequest) -> Result<Vec<u8>, Error> {
        let ControllerService::Started {
            cfg, simulation, ..
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let mut state = Vec::new();
        simulation
            .save_with_serialized_cfg(cfg.clone(), &mut state)
            .map_err(map_execution_error)
            .map(|_| state)
    }
}

impl fmt::Debug for ControllerService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControllerService").finish_non_exhaustive()
    }
}

/// An error returned when a the simulation time is out of the range supported
/// by gRPC.
fn final_simulation_time_error() -> Error {
    to_error(
        ErrorCode::SimulationTimeOutOfRange,
        "the final simulation time is out of range",
    )
}
