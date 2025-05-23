use std::fmt;
use std::sync::Arc;

use crate::registry::EventSourceRegistry;
use crate::server::key_registry::{KeyRegistry, KeyRegistryId};
use crate::simulation::Scheduler;

use super::super::codegen::simulation::*;
use super::{
    map_scheduling_error, monotonic_to_timestamp, simulation_not_started_error,
    timestamp_to_monotonic, to_error, to_strictly_positive_duration,
};

/// Protobuf-based simulation scheduler.
///
/// A `SchedulerService` enables the scheduling of simulation events.
#[allow(clippy::large_enum_variant)]
pub(crate) enum SchedulerService {
    NotStarted,
    Started {
        scheduler: Scheduler,
        event_source_registry: Arc<EventSourceRegistry>,
        key_registry: KeyRegistry,
    },
}

impl SchedulerService {
    /// Returns the current simulation time.
    pub(crate) fn time(&mut self, _request: TimeRequest) -> TimeReply {
        let reply = match self {
            Self::Started { scheduler, .. } => {
                if let Some(timestamp) = monotonic_to_timestamp(scheduler.time()) {
                    time_reply::Result::Time(timestamp)
                } else {
                    time_reply::Result::Error(to_error(
                        ErrorCode::SimulationTimeOutOfRange,
                        "the final simulation time is out of range",
                    ))
                }
            }
            Self::NotStarted => time_reply::Result::Error(simulation_not_started_error()),
        };

        TimeReply {
            result: Some(reply),
        }
    }

    /// Schedules an event at a future time.
    pub(crate) fn schedule_event(&mut self, request: ScheduleEventRequest) -> ScheduleEventReply {
        let reply = match self {
            Self::Started {
                scheduler,
                event_source_registry,
                key_registry,
            } => move || -> Result<Option<KeyRegistryId>, Error> {
                let source_name = &request.source_name;
                let event = &request.event;
                let with_key = request.with_key;
                let period = request
                    .period
                    .map(|period| {
                        to_strictly_positive_duration(period).ok_or(to_error(
                            ErrorCode::InvalidPeriod,
                            "the specified event period is not strictly positive",
                        ))
                    })
                    .transpose()?;

                let source = event_source_registry.get(source_name).ok_or(to_error(
                    ErrorCode::SourceNotFound,
                    "no event source is registered with the name '{}'".to_string(),
                ))?;

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

                let deadline = request.deadline.ok_or(to_error(
                    ErrorCode::MissingArgument,
                    "missing deadline argument",
                ))?;

                let deadline = match deadline {
                    schedule_event_request::Deadline::Time(time) => timestamp_to_monotonic(time)
                        .ok_or(to_error(
                            ErrorCode::InvalidTime,
                            "out-of-range nanosecond field",
                        ))?,
                    schedule_event_request::Deadline::Duration(duration) => {
                        let duration = to_strictly_positive_duration(duration).ok_or(to_error(
                            ErrorCode::InvalidDeadline,
                            "the specified scheduling deadline is not in the future",
                        ))?;

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
                    .map_err(map_scheduling_error)?;

                Ok(key_id)
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        ScheduleEventReply {
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
        }
    }

    /// Cancels a keyed event.
    pub(crate) fn cancel_event(&mut self, request: CancelEventRequest) -> CancelEventReply {
        let reply = match self {
            Self::Started {
                scheduler,
                key_registry,
                ..
            } => move || -> Result<(), Error> {
                let key = request
                    .key
                    .ok_or(to_error(ErrorCode::MissingArgument, "missing key argument"))?;
                let subkey1: usize = key
                    .subkey1
                    .try_into()
                    .map_err(|_| to_error(ErrorCode::InvalidKey, "invalid event key"))?;
                let subkey2 = key.subkey2;

                let key_id = KeyRegistryId::from_raw_parts(subkey1, subkey2);

                key_registry.remove_expired_keys(scheduler.time());
                let key = key_registry.extract_key(key_id).ok_or(to_error(
                    ErrorCode::InvalidKey,
                    "invalid or expired event key",
                ))?;

                key.cancel();

                Ok(())
            }(),
            Self::NotStarted => Err(simulation_not_started_error()),
        };

        CancelEventReply {
            result: Some(match reply {
                Ok(()) => cancel_event_reply::Result::Empty(()),
                Err(error) => cancel_event_reply::Result::Error(error),
            }),
        }
    }

    /// Requests an interruption of the simulation at the earliest opportunity.
    pub(crate) fn halt(&mut self, _request: HaltRequest) -> HaltReply {
        let reply = match self {
            Self::Started { scheduler, .. } => {
                scheduler.halt();

                halt_reply::Result::Empty(())
            }
            Self::NotStarted => halt_reply::Result::Error(simulation_not_started_error()),
        };

        HaltReply {
            result: Some(reply),
        }
    }
}

impl fmt::Debug for SchedulerService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchedulerService").finish_non_exhaustive()
    }
}
