use std::fmt;
use std::sync::{Arc, Mutex};
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
        event_sink_registry: Arc<Mutex<EventSinkRegistry>>,
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

                let mut sink =
                    event_sink_registry
                        .lock()
                        .unwrap()
                        .get(sink_name)
                        .ok_or(to_error(
                            ErrorCode::SinkNotFound,
                            format!("no sink is registered with the name '{sink_name}'"),
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

    /// Waits for an event from an event sink.
    pub(crate) fn await_event(&self, request: AwaitEventRequest) -> AwaitEventReply {
        let reply = match self {
            Self::Started {
                event_sink_registry,
            } => move || -> Result<Vec<u8>, Error> {
                let sink_name = &request.sink_name;

                let mut sink =
                    event_sink_registry
                        .lock()
                        .unwrap()
                        .get(sink_name)
                        .ok_or(to_error(
                            ErrorCode::SinkNotFound,
                            format!("no sink is registered with the name '{sink_name}'"),
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

                if let Some(sink) = event_sink_registry.lock().unwrap().get_mut(sink_name) {
                    sink.open();

                    open_sink_reply::Result::Empty(())
                } else {
                    open_sink_reply::Result::Error(to_error(
                        ErrorCode::SinkNotFound,
                        format!("no sink is registered with the name '{sink_name}'"),
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

                if let Some(sink) = event_sink_registry.lock().unwrap().get_mut(sink_name) {
                    sink.close();

                    close_sink_reply::Result::Empty(())
                } else {
                    close_sink_reply::Result::Error(to_error(
                        ErrorCode::SinkNotFound,
                        format!("no sink is registered with the name '{sink_name}'"),
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

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::ports::EventSinkReader;

    use super::super::assert_reply_err;
    use super::*;

    const U8_CBOR_HEADER: u8 = 0x18;
    const NULL_CBOR_HEADER: u8 = 0xf6;

    // TODO The fixtures here might be overengineered, reconsider.

    #[derive(Clone, Debug, Default)]
    struct DummyState<T> {
        queue: VecDeque<T>,
        open: bool,
        blocking: bool,
        timeout: Duration,
    }

    #[derive(Clone, Debug)]
    struct DummySink<T>(Arc<Mutex<DummyState<T>>>);
    impl<T: Default> DummySink<T> {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(DummyState {
                open: true,
                ..Default::default()
            })))
        }
    }
    impl<T> Iterator for DummySink<T> {
        type Item = T;
        fn next(&mut self) -> Option<T> {
            let mut state = self.0.lock().unwrap();
            if !state.blocking {
                return state.queue.pop_front();
            }
            drop(state);

            let start = Instant::now();
            loop {
                let mut state = self.0.lock().unwrap();
                let front = state.queue.pop_front();
                if front.is_some() {
                    break front;
                }
                if !state.timeout.is_zero() && start.elapsed() > state.timeout {
                    break None;
                }
                // Release the MutexGuard so the queue can be pushed into.
                drop(state);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    impl<T: Clone> EventSinkReader for DummySink<T> {
        fn open(&mut self) {
            self.0.lock().unwrap().open = true;
        }
        fn close(&mut self) {
            self.0.lock().unwrap().open = false;
        }
        fn set_blocking(&mut self, val: bool) {
            self.0.lock().unwrap().blocking = val;
        }
        fn set_timeout(&mut self, val: Duration) {
            self.0.lock().unwrap().timeout = val;
        }
    }

    #[derive(Default)]
    struct TestParams<'a> {
        event_sinks: Vec<&'a str>,
    }

    fn get_service(params: TestParams) -> (MonitorService, Vec<Arc<Mutex<DummyState<u8>>>>) {
        let mut event_sink_registry = EventSinkRegistry::default();
        let mut states = Vec::new();

        for sink_name in params.event_sinks {
            let sink = DummySink::new();
            states.push(sink.0.clone());
            event_sink_registry.add(sink, sink_name).unwrap();
        }

        (
            MonitorService::Started {
                event_sink_registry: Arc::new(Mutex::new(event_sink_registry)),
            },
            states,
        )
    }

    #[test]
    fn read_events() {
        let (service, states) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        // Should read events from `1` only.
        // Have to use values > 23 here, as CBOR encodes low uints with no header ;)
        states[0].lock().unwrap().queue.extend(&[48]);
        states[1].lock().unwrap().queue.extend(&[54, 57]);

        let reply = service.read_events(ReadEventsRequest {
            sink_name: "sink".to_string(),
        });

        assert_eq!(reply.result, Some(read_events_reply::Result::Empty(())));
        assert_eq!(
            reply.events,
            vec![vec![U8_CBOR_HEADER, 54], vec![U8_CBOR_HEADER, 57]]
        );
    }

    #[test]
    fn read_events_invalid_sink() {
        let (service, _) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        let reply = service.read_events(ReadEventsRequest {
            sink_name: "nonexsting".to_string(),
        });
        assert_reply_err!(reply, read_events_reply, ErrorCode::SinkNotFound);
    }

    #[test]
    fn read_events_not_started() {
        let service = MonitorService::NotStarted;
        let reply = service.read_events(ReadEventsRequest {
            sink_name: "sink".to_string(),
        });
        assert_reply_err!(reply, read_events_reply, ErrorCode::SimulationNotStarted);
    }

    #[test]
    fn await_event() {
        let (service, states) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_secs(1));
                states[1].lock().unwrap().queue.extend(&[48]);
            });
            let reply = service.await_event(AwaitEventRequest {
                sink_name: "sink".to_string(),
                timeout: Some(prost_types::Duration {
                    seconds: 5,
                    nanos: 0,
                }),
            });

            assert_eq!(
                reply.result,
                Some(await_event_reply::Result::Event(vec![U8_CBOR_HEADER, 48]))
            );
        });
    }

    #[test]
    fn await_not_started() {
        let service = MonitorService::NotStarted;
        let reply = service.await_event(AwaitEventRequest {
            sink_name: "sink".to_string(),
            timeout: None,
        });
        assert_reply_err!(reply, await_event_reply, ErrorCode::SimulationNotStarted);
    }

    #[test]
    fn await_event_timeout() {
        let (service, states) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_secs(1));
                states[1].lock().unwrap().queue.extend(&[48]);
            });

            let reply = service.await_event(AwaitEventRequest {
                sink_name: "sink".to_string(),
                timeout: Some(prost_types::Duration {
                    seconds: 0,
                    nanos: 5000,
                }),
            });

            assert_eq!(
                reply.result,
                Some(await_event_reply::Result::Event(vec![NULL_CBOR_HEADER]))
            );
        });
    }

    #[test]
    fn await_event_invalid_timeout() {
        let (service, _) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        let reply = service.await_event(AwaitEventRequest {
            sink_name: "sink".to_string(),
            timeout: Some(prost_types::Duration {
                seconds: 0,
                nanos: -5000,
            }),
        });
        assert_reply_err!(reply, await_event_reply, ErrorCode::InvalidTimeout);
    }

    #[test]
    fn await_event_invalid_sink() {
        let (service, _) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        let reply = service.await_event(AwaitEventRequest {
            sink_name: "nonexsiting".to_string(),
            timeout: Some(prost_types::Duration {
                seconds: 5,
                nanos: 0,
            }),
        });
        assert_reply_err!(reply, await_event_reply, ErrorCode::SinkNotFound);
    }

    #[test]
    fn close_and_open_sink() {
        let (mut service, states) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        let reply = service.close_sink(CloseSinkRequest {
            sink_name: "sink".to_string(),
        });
        assert_eq!(reply.result, Some(close_sink_reply::Result::Empty(())));
        // Check that only the requested sink has closed.
        assert!(states[0].lock().unwrap().open);
        assert!(!states[1].lock().unwrap().open);

        let reply = service.open_sink(OpenSinkRequest {
            sink_name: "sink".to_string(),
        });
        assert_eq!(reply.result, Some(open_sink_reply::Result::Empty(())));
        assert!(states[1].lock().unwrap().open);
    }

    #[test]
    fn close_invalid_sink() {
        let (mut service, _) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        let reply = service.close_sink(CloseSinkRequest {
            sink_name: "nonexisting".to_string(),
        });
        assert_reply_err!(reply, close_sink_reply, ErrorCode::SinkNotFound);
    }

    #[test]
    fn open_invalid_sink() {
        let (mut service, _) = get_service(TestParams {
            event_sinks: vec!["other", "sink"],
        });

        let reply = service.open_sink(OpenSinkRequest {
            sink_name: "nonexisting".to_string(),
        });
        assert_reply_err!(reply, open_sink_reply, ErrorCode::SinkNotFound);
    }

    #[test]
    fn close_sink_not_started() {
        let mut service = MonitorService::NotStarted;
        let reply = service.close_sink(CloseSinkRequest {
            sink_name: "sink".to_string(),
        });
        assert_reply_err!(reply, close_sink_reply, ErrorCode::SimulationNotStarted);
    }

    #[test]
    fn open_sink_not_started() {
        let mut service = MonitorService::NotStarted;
        let reply = service.open_sink(OpenSinkRequest {
            sink_name: "sink".to_string(),
        });
        assert_reply_err!(reply, open_sink_reply, ErrorCode::SimulationNotStarted);
    }
}
