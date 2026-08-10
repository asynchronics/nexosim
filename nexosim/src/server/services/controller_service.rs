use std::sync::Arc;

use prost_types::Timestamp;

#[cfg(feature = "tracing")]
use tracing::{debug, info};

use crate::endpoints::{EventSourceRegistry, QuerySourceRegistry};
use crate::path::Path as NexosimPath;
use crate::server::services::from_endpoint_error;
use crate::simulation::Simulation;

use super::super::codegen::simulation::*;
use super::{
    from_execution_error, monotonic_to_timestamp, simulation_not_started_error,
    timestamp_to_monotonic, to_error, to_positive_duration,
};

/// Protobuf-based simulation controller.
///
/// A `ControllerService` controls the execution of the simulation. Note that
/// all its methods block until execution completes.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ControllerService {
    Halted,
    Started {
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
            return Err(simulation_not_started_error());
        };

        #[cfg(feature = "tracing")]
        info!("simulation will to advance to a next scheduled event or tick");

        simulation
            .step()
            .map_err(from_execution_error)
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
            return Err(simulation_not_started_error());
        };

        let deadline = request
            .deadline
            .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing deadline argument"))?;

        match deadline {
            step_until_request::Deadline::Time(time) => {
                let time = timestamp_to_monotonic(time).ok_or_else(|| {
                    to_error(ErrorCode::InvalidTime, "out-of-range nanosecond field")
                })?;

                #[cfg(feature = "tracing")]
                info!("simulation will advance until: {:?}", deadline);

                simulation.step_until(time).map_err(from_execution_error)?;
            }
            step_until_request::Deadline::Duration(duration) => {
                let duration = to_positive_duration(duration).ok_or_else(|| {
                    to_error(
                        ErrorCode::InvalidDeadline,
                        "the specified deadline lies in the past",
                    )
                })?;

                #[cfg(feature = "tracing")]
                info!("simulation will advance by: {:?}", deadline);

                simulation
                    .step_until(duration)
                    .map_err(from_execution_error)?;
            }
        };

        monotonic_to_timestamp(simulation.time()).ok_or_else(final_simulation_time_error)
    }

    /// Iteratively advances the simulation time, as if by calling
    /// [`Simulation::step`] repeatedly.
    ///
    /// This method blocks until the simulation is halted or all scheduled
    /// events have completed.
    pub(crate) fn run(&mut self, _request: RunRequest) -> Result<Timestamp, Error> {
        let Self::Started { simulation, .. } = self else {
            return Err(simulation_not_started_error());
        };

        #[cfg(feature = "tracing")]
        info!("simulation will run indefinitely");

        simulation.run().map_err(from_execution_error)?;

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
            return Err(simulation_not_started_error());
        };

        let source_path: &NexosimPath = &request
            .source
            .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing event source path"))?
            .segments
            .into();
        let event = &request.event;

        let source = event_source_registry
            .get(source_path)
            .map_err(from_endpoint_error)?;

        let arg = source.deserialize_arg(event).map_err(|e| {
            to_error(
                ErrorCode::InvalidMessage,
                format!(
                    "the event '{}' could not be deserialized as type '{}': {}",
                    source_path,
                    source.event_type_name(),
                    e
                ),
            )
        })?;

        simulation
            .process_event_erased(source, arg)
            .map_err(from_execution_error)
            .inspect(|_| {
                #[cfg(feature = "tracing")]
                debug!("event '{source_path}' processed successfully");
            })
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
            return Err(simulation_not_started_error());
        };

        let source_path: &NexosimPath = &request
            .source
            .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing query source path"))?
            .segments
            .into();
        let request = &request.request;

        let source = query_source_registry
            .get(source_path)
            .map_err(from_endpoint_error)?;

        let arg = source.deserialize_arg(request).map_err(|e| {
            to_error(
                ErrorCode::InvalidMessage,
                format!(
                    "the query '{}' request could not be deserialized as type '{}': {}",
                    source_path,
                    source.request_type_name(),
                    e
                ),
            )
        })?;

        let mut rx = simulation
            .process_query_erased(source, arg)
            .map_err(from_execution_error)
            .inspect(|_| {
                #[cfg(feature = "tracing")]
                debug!("query '{source_path}' processed successfully, awaiting replies");
            })?;

        let replies = rx.take_collect().ok_or_else(|| to_error(
            ErrorCode::SimulationBadQuery,
            format!("a reply to the query '{source_path}' was expected but none was available; maybe the target model was not added to the simulation?"),
        ))?;

        replies
            .map_err(|e| {
                to_error(
                    ErrorCode::InvalidMessage,
                    format!(
                        "the query '{}' reply could not be serialized as type '{}': {}",
                        source_path,
                        source.reply_type_name(),
                        e
                    ),
                )
            })
            .inspect(|r| {
                #[cfg(feature = "tracing")]
                debug!(
                    "number of replies received for query '{}': {}",
                    source_path,
                    r.len()
                );
            })
    }

    /// Saves and returns current simulation state in a serialized form.
    pub(crate) fn save(&mut self, _: SaveRequest) -> Result<Vec<u8>, Error> {
        let ControllerService::Started { simulation, .. } = self else {
            return Err(simulation_not_started_error());
        };

        let mut state = Vec::new();
        simulation
            .save(&mut state)
            .map_err(from_execution_error)
            .map(|_| state)
            .inspect(|_| {
                #[cfg(feature = "tracing")]
                info!("simulation state serialized successfully");
            })
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
