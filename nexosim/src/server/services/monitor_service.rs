use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use crate::registry::EventSinkRegistry;

use super::super::codegen::simulation::*;
use super::{map_registry_error, simulation_not_started_error, to_error, to_positive_duration};

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

    /// Waits for an event from an event sink.
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

    pub(crate) fn list_event_sinks(&self, _: ListEventSinksRequest) -> ListEventSinksReply {
        match self {
            Self::Started {
                event_sink_registry,
                ..
            } => ListEventSinksReply {
                sink_names: event_sink_registry
                    .list_sinks()
                    .map(|a| a.to_string())
                    .collect(),
                result: Some(list_event_sinks_reply::Result::Empty(())),
            },
            Self::NotStarted => ListEventSinksReply {
                sink_names: Vec::new(),
                result: Some(list_event_sinks_reply::Result::Error(
                    simulation_not_started_error(),
                )),
            },
        }
    }

    pub(crate) fn get_event_sink_schemas(
        &self,
        request: GetEventSinkSchemasRequest,
    ) -> GetEventSinkSchemasReply {
        match self {
            Self::Started {
                event_sink_registry,
                ..
            } => {
                let schemas = if request.sink_names.is_empty() {
                    event_sink_registry
                        .list_sinks()
                        .map(|a| Ok((a.to_string(), event_sink_registry.get_sink_schema(a)?)))
                        .collect()
                } else {
                    request
                        .sink_names
                        .iter()
                        .map(|a| Ok((a.to_string(), event_sink_registry.get_sink_schema(a)?)))
                        .collect()
                };
                match schemas {
                    Ok(schemas) => GetEventSinkSchemasReply {
                        schemas,
                        result: Some(get_event_sink_schemas_reply::Result::Empty(())),
                    },
                    Err(e) => GetEventSinkSchemasReply {
                        schemas: HashMap::new(),
                        result: Some(get_event_sink_schemas_reply::Result::Error(
                            map_registry_error(e),
                        )),
                    },
                }
            }
            Self::NotStarted => GetEventSinkSchemasReply {
                schemas: HashMap::new(),
                result: Some(get_event_sink_schemas_reply::Result::Error(
                    simulation_not_started_error(),
                )),
            },
        }
    }
}

impl fmt::Debug for MonitorService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimulationService").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use crate::ports::EventSinkReader;

    use super::*;

    #[derive(Clone, Debug)]
    struct DummySink<T>(T);
    impl<T> DummySink<T> {
        fn new(t: T) -> Self {
            Self(t)
        }
    }
    impl<T> Iterator for DummySink<T> {
        type Item = ();
        fn next(&mut self) -> Option<()> {
            None
        }
    }
    impl<T: Clone> EventSinkReader for DummySink<T> {
        fn open(&mut self) {}
        fn close(&mut self) {}
        fn set_blocking(&mut self, _: bool) {}
        fn set_timeout(&mut self, _: Duration) {}
    }

    fn get_service<'a>(sinks: impl IntoIterator<Item = &'a str>) -> MonitorService {
        let mut event_sink_registry = EventSinkRegistry::default();
        for sink in sinks {
            event_sink_registry.add(DummySink::new(()), sink).unwrap();
        }
        MonitorService::Started {
            event_sink_registry,
        }
    }

    #[test]
    fn get_single_schema() {
        let reply =
            get_service(["main", "other"]).get_event_sink_schemas(GetEventSinkSchemasRequest {
                sink_names: vec!["main".to_string()],
            });
        assert_eq!(
            reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(reply.schemas.len(), 1);
        assert_eq!(reply.schemas.keys().next().unwrap(), "main");
    }

    #[test]
    fn get_multiple_schemas() {
        let reply = get_service(["main", "secondary", "other"]).get_event_sink_schemas(
            GetEventSinkSchemasRequest {
                sink_names: vec!["main".to_string(), "secondary".to_string()],
            },
        );
        assert_eq!(
            reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(reply.schemas.len(), 2);
        let keys = reply
            .schemas
            .into_keys()
            .collect::<std::collections::HashSet<String>>();
        assert!(keys.contains("main"));
        assert!(keys.contains("secondary"));
    }

    #[test]
    fn get_all_schemas() {
        let reply = get_service(["main", "secondary"])
            .get_event_sink_schemas(GetEventSinkSchemasRequest { sink_names: vec![] });

        assert_eq!(
            reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(reply.schemas.len(), 2);
    }

    #[test]
    fn get_missing_schema() {
        let reply = get_service(["main"]).get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["main".to_string(), "secondary".to_string()],
        });
        assert!(matches!(
            reply.result,
            Some(get_event_sink_schemas_reply::Result::Error(_))
        ));
        assert!(reply.schemas.is_empty());
    }

    #[test]
    fn get_empty_schema() {
        let mut event_sink_registry = EventSinkRegistry::default();
        event_sink_registry
            .add_raw(DummySink::new(()), "main")
            .unwrap();
        let service = MonitorService::Started {
            event_sink_registry,
        };

        let reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["main".to_string()],
        });

        assert_eq!(
            reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(reply.schemas.len(), 1);
        assert_eq!(reply.schemas.keys().next().unwrap(), "main");
        assert_eq!(reply.schemas.values().next().unwrap(), "");
    }
}
