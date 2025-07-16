use std::fmt;
use std::sync::Arc;

use prost_types::Timestamp;

use crate::registry::{EventSourceRegistry, QuerySourceRegistry};
use crate::simulation::Simulation;

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

                let action = source.action(event).map_err(|e| {
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
                    .process_action(action)
                    .map_err(map_execution_error)
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

                let (action, mut receiver) = source.query(request).map_err(|e| {
                    to_error(
                        ErrorCode::InvalidMessage,
                        format!(
                            "the request could not be deserialized as type '{}': {}",
                            source.request_type_name(),
                            e
                        ),
                    )
                })?;

                simulation
                    .process_action(action)
                    .map_err(map_execution_error)?;

                let replies = receiver.take_collect().ok_or(to_error(
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

    /// Saves and returns current simulation state in a serialized form.
    pub(crate) fn save(&mut self, _: SaveRequest) -> SaveReply {
        let ControllerService::Started {
            cfg, simulation, ..
        } = self
        else {
            return SaveReply {
                result: Some(save_reply::Result::Error(to_error(
                    ErrorCode::SimulationNotStarted,
                    "the simulation has not been started",
                ))),
            };
        };

        let mut state = Vec::new();
        let result = simulation
            .save_with_serialized_cfg(cfg.clone(), &mut state)
            .map_err(|_| {
                crate::simulation::ExecutionError::SaveError(
                    "Simulation config serialization has failed.".to_string(),
                )
            });

        let result = match result {
            Err(e) => save_reply::Result::Error(map_execution_error(e)),
            Ok(_) => save_reply::Result::State(state),
        };

        SaveReply {
            result: Some(result),
        }
    }
}

impl fmt::Debug for ControllerService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControllerService").finish_non_exhaustive()
    }
}

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tai_time::TaiTime;

    use crate as nexosim;
    use crate::ports::{EventSource, QuerySource};
    use crate::simulation::{Mailbox, SimInit};
    use crate::Model;

    use super::*;

    const U8_CBOR_HEADER: u8 = 0x18;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct DummyModel {
        #[serde(skip)]
        value: Arc<AtomicU8>,
    }
    #[Model]
    impl DummyModel {
        fn input(&mut self, arg: u8) {
            self.value
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |a| Some(a + arg))
                .unwrap();
        }
        async fn query(&mut self, arg: u8) -> u8 {
            7 * arg
        }
    }

    #[derive(Default)]
    struct TestParams<'a> {
        event_sources: Vec<&'a str>,
        query_sources: Vec<&'a str>,
    }

    fn get_service(params: TestParams) -> (ControllerService, Vec<Arc<AtomicU8>>) {
        let mut bench = SimInit::new();
        let mut state = Vec::new();

        let mut event_source_registry = EventSourceRegistry::default();
        for source_name in params.event_sources {
            let value = Arc::new(AtomicU8::new(0));
            state.push(value.clone());
            let model = DummyModel { value };
            let mbox = Mailbox::new();

            let mut source = EventSource::new();
            source.connect(DummyModel::input, mbox.address());
            event_source_registry
                .add::<u8>(source, source_name)
                .unwrap();

            bench = bench.add_model(model, mbox, "Model");
        }
        event_source_registry.register_scheduler(bench.scheduler_registry());

        let mut query_source_registry = QuerySourceRegistry::default();
        for source_name in params.query_sources {
            let model = DummyModel {
                value: Arc::new(AtomicU8::new(0)),
            };
            let mbox = Mailbox::new();
            let mut source = QuerySource::new();
            source.connect(DummyModel::query, mbox.address());

            query_source_registry
                .add::<u8, u8>(source, source_name)
                .unwrap();

            bench = bench.add_model(model, mbox, "Model");
        }

        (
            ControllerService::Started {
                cfg: vec![],
                simulation: bench.init(TaiTime::from_unix_timestamp(0, 0, 0)).unwrap(),
                event_source_registry: Arc::new(event_source_registry),
                query_source_registry: Arc::new(query_source_registry),
            },
            state,
        )
    }

    #[test]
    fn step() {
        let (mut service, state) = get_service(TestParams {
            event_sources: vec!["input"],
            ..Default::default()
        });

        if let ControllerService::Started {
            ref mut simulation,
            ref event_source_registry,
            ..
        } = service
        {
            let source = event_source_registry.get("input").unwrap();
            let event = source.event(&[U8_CBOR_HEADER, 47]).unwrap();
            simulation
                .scheduler()
                .schedule(Duration::from_secs(3), event)
                .unwrap();
        } else {
            panic!("Service is not started!");
        }

        let reply = service.step(StepRequest {});
        assert_eq!(
            reply.result,
            Some(step_reply::Result::Time(prost_types::Timestamp {
                seconds: 3,
                nanos: 0
            }))
        );
        if let ControllerService::Started { ref simulation, .. } = service {
            assert_eq!(simulation.time(), TaiTime::from_unix_timestamp(3, 0, 0));
        } else {
            panic!("Service is not started!");
        }
        // The event should be fired once.
        assert_eq!(state[0].load(Ordering::Relaxed), 47);
    }

    #[test]
    fn step_until() {
        let (mut service, state) = get_service(TestParams {
            event_sources: vec!["input"],
            ..Default::default()
        });

        if let ControllerService::Started {
            ref mut simulation,
            ref event_source_registry,
            ..
        } = service
        {
            let source = event_source_registry.get("input").unwrap();
            let event = source.event(&[U8_CBOR_HEADER, 47]).unwrap();
            simulation
                .scheduler()
                .schedule(Duration::from_secs(3), event)
                .unwrap();
        } else {
            panic!("Service is not started!");
        }

        let reply = service.step_until(StepUntilRequest {
            deadline: Some(step_until_request::Deadline::Duration(
                prost_types::Duration {
                    seconds: 2,
                    nanos: 0,
                },
            )),
        });
        assert_eq!(
            reply.result,
            Some(step_until_reply::Result::Time(prost_types::Timestamp {
                seconds: 2,
                nanos: 0
            }))
        );
        if let ControllerService::Started { ref simulation, .. } = service {
            assert_eq!(simulation.time(), TaiTime::from_unix_timestamp(2, 0, 0));
        } else {
            panic!("Service is not started!");
        }
        // The scheduled event should not get fired.
        assert_eq!(state[0].load(Ordering::Relaxed), 0);
    }

    #[test]
    fn step_unbounded() {
        let (mut service, state) = get_service(TestParams {
            event_sources: vec!["input"],
            ..Default::default()
        });

        if let ControllerService::Started {
            ref mut simulation,
            ref event_source_registry,
            ..
        } = service
        {
            let source = event_source_registry.get("input").unwrap();
            for i in 1..=5 {
                let event = source.event(&[U8_CBOR_HEADER, 30]).unwrap();
                simulation
                    .scheduler()
                    .schedule(Duration::from_secs(3 * i), event)
                    .unwrap();
            }
        } else {
            panic!("Service is not started!");
        }

        let reply = service.step_unbounded(StepUnboundedRequest {});
        assert_eq!(
            reply.result,
            Some(step_unbounded_reply::Result::Time(prost_types::Timestamp {
                seconds: 15,
                nanos: 0
            }))
        );
        if let ControllerService::Started { ref simulation, .. } = service {
            assert_eq!(simulation.time(), TaiTime::from_unix_timestamp(15, 0, 0));
        } else {
            panic!("Service is not started!");
        }
        // The event should be fired 5 times.
        assert_eq!(state[0].load(Ordering::Relaxed), 150);
    }

    #[test]
    fn process_event() {
        let (mut service, state) = get_service(TestParams {
            event_sources: vec!["other", "input"],
            ..Default::default()
        });

        let reply = service.process_event(ProcessEventRequest {
            source_name: "input".to_string(),
            event: vec![U8_CBOR_HEADER, 49],
        });
        assert_eq!(reply.result, Some(process_event_reply::Result::Empty(())));
        if let ControllerService::Started { ref simulation, .. } = service {
            // Fires immediately, simulation time does not advance.
            assert_eq!(simulation.time(), TaiTime::from_unix_timestamp(0, 0, 0));
        } else {
            panic!("Service is not started!");
        }
        assert_eq!(state[0].load(Ordering::Relaxed), 0);
        assert_eq!(state[1].load(Ordering::Relaxed), 49);
    }

    #[test]
    fn process_query() {
        let (mut service, _) = get_service(TestParams {
            query_sources: vec!["input"],
            ..Default::default()
        });

        let reply = service.process_query(ProcessQueryRequest {
            source_name: "input".to_string(),
            request: vec![U8_CBOR_HEADER, 11],
        });
        assert_eq!(reply.result, Some(process_query_reply::Result::Empty(())));
        assert_eq!(reply.replies, vec![vec![U8_CBOR_HEADER, 77]]);

        if let ControllerService::Started { ref simulation, .. } = service {
            // Fires immediately, simulation time does not advance.
            assert_eq!(simulation.time(), TaiTime::from_unix_timestamp(0, 0, 0));
        } else {
            panic!("Service is not started!");
        }
    }

    #[test]
    fn save() {
        let (mut service, _) = get_service(TestParams {
            event_sources: vec!["input"],
            ..Default::default()
        });

        if let ControllerService::Started {
            ref mut simulation,
            ref event_source_registry,
            ..
        } = service
        {
            // Let's schedule something to populate the queue.
            let source = event_source_registry.get("input").unwrap();
            let event = source.event(&[U8_CBOR_HEADER, 47]).unwrap();
            simulation
                .scheduler()
                .schedule(Duration::from_secs(3), event)
                .unwrap();
        } else {
            panic!("Service is not started!");
        }

        let reply = service.save(SaveRequest {});

        if let Some(save_reply::Result::State(state)) = reply.result {
            if let ControllerService::Started {
                ref mut simulation, ..
            } = service
            {
                // Verify that a valid state has been received.
                simulation.restore(&state[..]).unwrap();
            } else {
                panic!("Service is not started!")
            }
        } else {
            panic!("Invalid reply!");
        }
    }
}
