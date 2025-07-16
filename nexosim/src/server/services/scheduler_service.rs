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

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;
    use tai_time::TaiTime;

    use crate::ports::EventSource;
    use crate::server::key_registry::KeyRegistryId;

    use super::*;

    const U8_CBOR_HEADER: u8 = 0x18;

    #[derive(Default)]
    struct TestParams<'a> {
        event_sources: Vec<&'a str>,
    }

    fn get_service(params: TestParams) -> SchedulerService {
        let mut event_source_registry = EventSourceRegistry::default();
        for source in params.event_sources {
            event_source_registry
                .add::<u8>(EventSource::new(), source)
                .unwrap();
        }

        let mut scheduler_registry = crate::simulation::SchedulerSourceRegistry::default();
        event_source_registry.register_scheduler(&mut scheduler_registry);

        let scheduler = Scheduler::new_dummy();
        let key_registry = KeyRegistry::default();

        SchedulerService::Started {
            scheduler,
            event_source_registry: Arc::new(event_source_registry),
            key_registry,
        }
    }

    #[test]
    fn time() {
        let mut service = get_service(TestParams::default());
        let reply = service.time(TimeRequest {});

        if let SchedulerService::Started { scheduler, .. } = service {
            // TODO this test is not very strict as it is currently testing on a default
            // time.
            let time = scheduler.time();
            assert_eq!(
                reply.result,
                Some(time_reply::Result::Time(prost_types::Timestamp {
                    seconds: time.as_secs(),
                    nanos: time.subsec_nanos() as i32
                }))
            );
        } else {
            panic!("Scheduler service is not started!");
        }
    }

    #[test]
    fn schedule_event() {
        let mut service = get_service(TestParams {
            event_sources: vec!["input"],
        });

        let reply = service.schedule_event(ScheduleEventRequest {
            source_name: "input".to_string(),
            event: vec![U8_CBOR_HEADER, 13],
            period: None,
            with_key: false,
            deadline: Some(schedule_event_request::Deadline::Duration(
                prost_types::Duration {
                    seconds: 2,
                    nanos: 0,
                },
            )),
        });

        assert_eq!(reply.result, Some(schedule_event_reply::Result::Empty(())));

        if let SchedulerService::Started { scheduler, .. } = service {
            let queue = scheduler.queue();
            let entry = queue.peek().unwrap();
            assert_eq!(entry.0 .0, TaiTime::from_unix_timestamp(2, 0, 0));
            assert_eq!(*entry.1.arg.downcast_ref::<u8>().unwrap(), 13);
        } else {
            panic!();
        }
    }

    #[test]
    fn schedule_keyed_event() {
        let mut service = get_service(TestParams {
            event_sources: vec!["input"],
        });

        let reply = service.schedule_event(ScheduleEventRequest {
            source_name: "input".to_string(),
            event: vec![U8_CBOR_HEADER, 13],
            period: None,
            with_key: true,
            deadline: Some(schedule_event_request::Deadline::Duration(
                prost_types::Duration {
                    seconds: 2,
                    nanos: 0,
                },
            )),
        });

        let key_handle = if let Some(schedule_event_reply::Result::Key(key)) = reply.result {
            key
        } else {
            panic!("Invalid response - no key!");
        };

        if let SchedulerService::Started {
            scheduler,
            mut key_registry,
            ..
        } = service
        {
            let queue = scheduler.queue();
            let entry = queue.peek().unwrap();
            let key = key_registry
                .extract_key(KeyRegistryId::from_raw_parts(
                    key_handle.subkey1 as usize,
                    key_handle.subkey2,
                ))
                .unwrap();
            assert_eq!(entry.0 .0, TaiTime::from_unix_timestamp(2, 0, 0));
            assert_eq!(*entry.1.arg.downcast_ref::<u8>().unwrap(), 13);
            assert_eq!(entry.1.key.as_ref().unwrap().clone(), key);
        } else {
            panic!("Scheduler service is not started!");
        }
    }

    #[test]
    fn schedule_periodic_event() {
        let mut service = get_service(TestParams {
            event_sources: vec!["input"],
        });

        let reply = service.schedule_event(ScheduleEventRequest {
            source_name: "input".to_string(),
            event: vec![U8_CBOR_HEADER, 13],
            period: Some(prost_types::Duration {
                seconds: 5,
                nanos: 0,
            }),
            with_key: false,
            deadline: Some(schedule_event_request::Deadline::Duration(
                prost_types::Duration {
                    seconds: 2,
                    nanos: 0,
                },
            )),
        });

        assert_eq!(reply.result, Some(schedule_event_reply::Result::Empty(())));

        if let SchedulerService::Started { scheduler, .. } = service {
            let queue = scheduler.queue();
            let entry = queue.peek().unwrap();
            assert_eq!(entry.0 .0, TaiTime::from_unix_timestamp(2, 0, 0));
            assert_eq!(*entry.1.arg.downcast_ref::<u8>().unwrap(), 13);
            assert_eq!(entry.1.period.unwrap(), Duration::from_secs(5));
        } else {
            panic!("Scheduler service is not started!");
        }
    }

    #[test]
    fn schedule_keyed_periodic_event() {
        let mut service = get_service(TestParams {
            event_sources: vec!["input"],
        });

        let reply = service.schedule_event(ScheduleEventRequest {
            source_name: "input".to_string(),
            event: vec![U8_CBOR_HEADER, 13],
            period: Some(prost_types::Duration {
                seconds: 5,
                nanos: 0,
            }),
            with_key: true,
            deadline: Some(schedule_event_request::Deadline::Duration(
                prost_types::Duration {
                    seconds: 2,
                    nanos: 0,
                },
            )),
        });

        let key_handle = if let Some(schedule_event_reply::Result::Key(key)) = reply.result {
            key
        } else {
            panic!("Invalid response - no key!");
        };

        if let SchedulerService::Started {
            scheduler,
            mut key_registry,
            ..
        } = service
        {
            let queue = scheduler.queue();
            let entry = queue.peek().unwrap();
            let key = key_registry
                .extract_key(KeyRegistryId::from_raw_parts(
                    key_handle.subkey1 as usize,
                    key_handle.subkey2,
                ))
                .unwrap();
            assert_eq!(entry.0 .0, TaiTime::from_unix_timestamp(2, 0, 0));
            assert_eq!(*entry.1.arg.downcast_ref::<u8>().unwrap(), 13);
            assert_eq!(entry.1.key.as_ref().unwrap().clone(), key);
            assert_eq!(entry.1.period.unwrap(), Duration::from_secs(5));
        } else {
            panic!("Scheduler service is not started!");
        }
    }

    #[test]
    fn cancel_event() {
        let mut service = get_service(TestParams {
            event_sources: vec!["input"],
        });

        let key_handle = if let SchedulerService::Started {
            ref scheduler,
            ref mut key_registry,
            ref event_source_registry,
        } = service
        {
            let source = event_source_registry.get("input").unwrap();
            let (event, key) = source.keyed_event(&[U8_CBOR_HEADER, 17]).unwrap();
            scheduler.schedule(Duration::from_secs(2), event).unwrap();
            key_registry.insert_eternal_key(key)
        } else {
            panic!("Scheduler service is not started!");
        };

        let key_parts = key_handle.into_raw_parts();
        let reply = service.cancel_event(CancelEventRequest {
            key: Some(EventKey {
                subkey1: key_parts.0 as u64,
                subkey2: key_parts.1,
            }),
        });
        assert_eq!(reply.result, Some(cancel_event_reply::Result::Empty(())));

        if let SchedulerService::Started { scheduler, .. } = service {
            let queue = scheduler.queue();
            let entry = queue.peek().unwrap();
            assert!(entry.1.is_cancelled());
        } else {
            panic!("Scheduler service is not started!");
        }
    }

    #[test]
    fn halt() {
        let mut service = get_service(TestParams::default());

        let reply = service.halt(HaltRequest {});
        assert_eq!(reply.result, Some(halt_reply::Result::Empty(())));

        if let SchedulerService::Started { scheduler, .. } = service {
            assert!(scheduler.is_halted().load(Ordering::Relaxed));
        } else {
            panic!("Scheduler service is not started!");
        }
    }
}
