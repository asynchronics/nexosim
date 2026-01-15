use std::sync::Arc;

use prost_types::Timestamp;

use crate::endpoints::EventSourceRegistry;
use crate::path::Path as NexosimPath;
use crate::server::key_registry::{KeyRegistry, KeyRegistryId};
use crate::server::services::from_endpoint_error;
use crate::simulation::Scheduler;

use super::super::codegen::simulation::*;
use super::{
    from_scheduling_error, monotonic_to_timestamp, simulation_halted_error, timestamp_to_monotonic,
    to_error, to_strictly_positive_duration,
};

/// Protobuf-based simulation scheduler.
///
/// A `SchedulerService` enables the scheduling of simulation events.
#[allow(clippy::large_enum_variant)]
pub(crate) enum SchedulerService {
    Halted,
    Started {
        scheduler: Scheduler,
        event_source_registry: Arc<EventSourceRegistry>,
        key_registry: KeyRegistry,
    },
}

impl SchedulerService {
    /// Returns the current simulation time.
    pub(crate) fn time(&self, _request: TimeRequest) -> Result<Timestamp, Error> {
        match self {
            Self::Started { scheduler, .. } => {
                if let Some(timestamp) = monotonic_to_timestamp(scheduler.time()) {
                    Ok(timestamp)
                } else {
                    Err(to_error(
                        ErrorCode::SimulationTimeOutOfRange,
                        "the final simulation time is out of range",
                    ))
                }
            }
            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Requests an interruption of the simulation at the earliest opportunity.
    pub(crate) fn halt(&self, _request: HaltRequest) -> Result<(), Error> {
        match self {
            Self::Started { scheduler, .. } => {
                scheduler.halt();

                Ok(())
            }
            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Schedules an event at a future time.
    pub(crate) fn schedule_event(
        &mut self,
        request: ScheduleEventRequest,
    ) -> Result<Option<KeyRegistryId>, Error> {
        let Self::Started {
            scheduler,
            event_source_registry,
            key_registry,
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let source_path: &NexosimPath = &request
            .source
            .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing event source path"))?
            .segments
            .into();
        let event = &request.event;
        let with_key = request.with_key;
        let period = request
            .period
            .map(|period| {
                to_strictly_positive_duration(period).ok_or_else(|| {
                    to_error(
                        ErrorCode::InvalidPeriod,
                        "the specified event period is not strictly positive",
                    )
                })
            })
            .transpose()?;

        let source = event_source_registry
            .get(source_path)
            .map_err(from_endpoint_error)?;

        let (event, event_key) = match (with_key, period) {
            (false, None) => source.event(event).map(|action| (action, None)),
            (false, Some(period)) => source
                .periodic_event(period, event)
                .map(|action| (action, None)),
            (true, None) => source
                .keyed_event(event)
                .map(|(action, key)| (action, Some(key))),
            (true, Some(period)) => source
                .keyed_periodic_event(period, event)
                .map(|(action, key)| (action, Some(key))),
        }
        .map_err(|e| {
            to_error(
                ErrorCode::InvalidMessage,
                format!(
                    "the event could not be deserialized as type '{}': {}",
                    source.event_type_name(),
                    e
                ),
            )
        })?;

        let deadline = request
            .deadline
            .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing deadline argument"))?;

        let deadline = match deadline {
            schedule_event_request::Deadline::Time(time) => timestamp_to_monotonic(time)
                .ok_or_else(|| to_error(ErrorCode::InvalidTime, "out-of-range nanosecond field"))?,
            schedule_event_request::Deadline::Duration(duration) => {
                let duration = to_strictly_positive_duration(duration).ok_or_else(|| {
                    to_error(
                        ErrorCode::InvalidDeadline,
                        "the specified scheduling deadline is not in the future",
                    )
                })?;

                scheduler.time() + duration
            }
        };

        let key_id = event_key.map(|event_key| {
            key_registry.remove_expired_keys(scheduler.time());

            if period.is_some() {
                key_registry.insert_eternal_key(event_key)
            } else {
                key_registry.insert_key(event_key, deadline)
            }
        });

        scheduler
            .schedule(deadline, event)
            .map_err(from_scheduling_error)?;

        Ok(key_id)
    }

    /// Cancels a keyed event.
    pub(crate) fn cancel_event(&mut self, request: CancelEventRequest) -> Result<(), Error> {
        let Self::Started {
            scheduler,
            key_registry,
            ..
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let key = request
            .key
            .ok_or_else(|| to_error(ErrorCode::MissingArgument, "missing key argument"))?;
        let subkey1: usize = key
            .subkey1
            .try_into()
            .map_err(|_| to_error(ErrorCode::InvalidKey, "invalid event key"))?;
        let subkey2 = key.subkey2;

        let key_id = KeyRegistryId::from_raw_parts(subkey1, subkey2);

        key_registry.remove_expired_keys(scheduler.time());
        let key = key_registry
            .extract_key(key_id)
            .ok_or_else(|| to_error(ErrorCode::InvalidKey, "invalid or expired event key"))?;

        key.cancel();

        Ok(())
    }
}
