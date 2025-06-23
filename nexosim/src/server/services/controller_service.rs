use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use prost_types::Timestamp;

use crate::registry::{EventSourceRegistry, QuerySourceRegistry};
use crate::simulation::Simulation;

use super::super::codegen::simulation::*;
use super::{
    map_execution_error, map_registry_error, monotonic_to_timestamp, simulation_not_started_error,
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

    pub(crate) fn list_event_sources(
        &mut self,
        _: ListEventSourcesRequest,
    ) -> ListEventSourcesReply {
        match self {
            Self::Started {
                event_source_registry,
                ..
            } => ListEventSourcesReply {
                source_names: event_source_registry
                    .list_sources()
                    .map(|a| a.to_string())
                    .collect(),
                result: Some(list_event_sources_reply::Result::Empty(())),
            },
            Self::NotStarted => ListEventSourcesReply {
                source_names: Vec::new(),
                result: Some(list_event_sources_reply::Result::Error(
                    simulation_not_started_error(),
                )),
            },
        }
    }

    pub(crate) fn get_event_source_schemas(
        &mut self,
        request: GetEventSourceSchemasRequest,
    ) -> GetEventSourceSchemasReply {
        match self {
            Self::Started {
                event_source_registry,
                ..
            } => {
                let schemas = if request.source_names.is_empty() {
                    // When no argument is provided return all the schemas.
                    event_source_registry
                        .list_sources()
                        .map(|a| Ok((a.to_string(), event_source_registry.get_source_schema(a)?)))
                        .collect()
                } else {
                    request
                        .source_names
                        .iter()
                        .map(|a| Ok((a.to_string(), event_source_registry.get_source_schema(a)?)))
                        .collect()
                };

                match schemas {
                    Ok(schemas) => GetEventSourceSchemasReply {
                        schemas,
                        result: Some(get_event_source_schemas_reply::Result::Empty(())),
                    },
                    Err(e) => GetEventSourceSchemasReply {
                        schemas: HashMap::new(),
                        result: Some(get_event_source_schemas_reply::Result::Error(
                            map_registry_error(e),
                        )),
                    },
                }
            }
            Self::NotStarted => GetEventSourceSchemasReply {
                schemas: HashMap::new(),
                result: Some(get_event_source_schemas_reply::Result::Error(
                    simulation_not_started_error(),
                )),
            },
        }
    }

    pub(crate) fn list_query_sources(
        &mut self,
        _: ListQuerySourcesRequest,
    ) -> ListQuerySourcesReply {
        match self {
            Self::Started {
                query_source_registry,
                ..
            } => ListQuerySourcesReply {
                source_names: query_source_registry
                    .list_sources()
                    .map(|a| a.to_string())
                    .collect(),
                result: Some(list_query_sources_reply::Result::Empty(())),
            },
            Self::NotStarted => ListQuerySourcesReply {
                source_names: Vec::new(),
                result: Some(list_query_sources_reply::Result::Error(
                    simulation_not_started_error(),
                )),
            },
        }
    }

    pub(crate) fn get_query_source_schemas(
        &mut self,
        request: GetQuerySourceSchemasRequest,
    ) -> GetQuerySourceSchemasReply {
        match self {
            Self::Started {
                query_source_registry,
                ..
            } => {
                let schemas = if request.source_names.is_empty() {
                    // When no argument is provided return all the schemas.
                    query_source_registry
                        .list_sources()
                        .map(|a| {
                            let schema = query_source_registry.get_source_schema(a)?;
                            Ok((
                                a.to_string(),
                                QuerySchema {
                                    input: schema.0,
                                    output: schema.1,
                                },
                            ))
                        })
                        .collect()
                } else {
                    request
                        .source_names
                        .iter()
                        .map(|a| {
                            let schema = query_source_registry.get_source_schema(a)?;
                            Ok((
                                a.to_string(),
                                QuerySchema {
                                    input: schema.0,
                                    output: schema.1,
                                },
                            ))
                        })
                        .collect()
                };

                match schemas {
                    Ok(schemas) => GetQuerySourceSchemasReply {
                        schemas,
                        result: Some(get_query_source_schemas_reply::Result::Empty(())),
                    },
                    Err(e) => GetQuerySourceSchemasReply {
                        schemas: HashMap::new(),
                        result: Some(get_query_source_schemas_reply::Result::Error(
                            map_registry_error(e),
                        )),
                    },
                }
            }
            Self::NotStarted => GetQuerySourceSchemasReply {
                schemas: HashMap::new(),
                result: Some(get_query_source_schemas_reply::Result::Error(
                    simulation_not_started_error(),
                )),
            },
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

                let event = source.event(event).map_err(|e| {
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

                let (query, mut promise) = source.query(request).map_err(|e| {
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
}

impl fmt::Debug for ControllerService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControllerService").finish_non_exhaustive()
    }
}

#[cfg(all(test, not(nexosim_loom)))]
mod tests {

    use super::*;

    use crate::ports::{EventSource, QuerySource};

    fn get_service(
        event_sources: Vec<&str>,
        raw_event_sources: Vec<&str>,
        query_sources: Vec<&str>,
        raw_query_sources: Vec<&str>,
    ) -> ControllerService {
        let mut event_source_registry = EventSourceRegistry::default();
        for source in event_sources {
            event_source_registry
                .add::<()>(EventSource::new(), source)
                .unwrap();
        }
        for source in raw_event_sources {
            event_source_registry
                .add_raw::<()>(EventSource::new(), source)
                .unwrap();
        }

        let mut query_source_registry = QuerySourceRegistry::default();
        for source in query_sources {
            query_source_registry
                .add::<(), ()>(QuerySource::new(), source)
                .unwrap();
        }
        for source in raw_query_sources {
            query_source_registry
                .add_raw::<(), ()>(QuerySource::new(), source)
                .unwrap();
        }

        ControllerService::Started {
            simulation: Simulation::new_dummy(),
            event_source_registry: Arc::new(event_source_registry),
            query_source_registry,
        }
    }

    #[test]
    fn get_single_schemas() {
        let mut service = get_service(
            vec!["event", "other"],
            vec![],
            vec!["query", "other"],
            vec![],
        );
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["event".to_string()],
        });
        assert_eq!(
            event_reply.result,
            Some(get_event_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(event_reply.schemas.len(), 1);
        assert_eq!(event_reply.schemas.keys().next().unwrap(), "event");

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["query".to_string()],
        });
        assert_eq!(
            query_reply.result,
            Some(get_query_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(query_reply.schemas.len(), 1);
        assert_eq!(query_reply.schemas.keys().next().unwrap(), "query");
    }

    #[test]
    fn get_multiple_schemas() {
        let mut service = get_service(
            vec!["event", "secondary", "other"],
            vec![],
            vec!["query", "secondary", "other"],
            vec![],
        );
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["event".to_string(), "secondary".to_string()],
        });
        assert_eq!(
            event_reply.result,
            Some(get_event_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(event_reply.schemas.len(), 2);
        let event_keys = event_reply
            .schemas
            .into_keys()
            .collect::<std::collections::HashSet<String>>();
        assert!(event_keys.contains("event"));
        assert!(event_keys.contains("secondary"));

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["query".to_string(), "secondary".to_string()],
        });
        assert_eq!(
            query_reply.result,
            Some(get_query_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(query_reply.schemas.len(), 2);
        let query_keys = query_reply
            .schemas
            .into_keys()
            .collect::<std::collections::HashSet<String>>();
        assert!(query_keys.contains("query"));
        assert!(query_keys.contains("secondary"));
    }

    #[test]
    fn get_all_schemas() {
        let mut service = get_service(
            vec!["event", "other"],
            vec!["raw"],
            vec!["query", "other"],
            vec!["raw"],
        );
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec![],
        });
        assert_eq!(
            event_reply.result,
            Some(get_event_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(event_reply.schemas.len(), 3);

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec![],
        });
        assert_eq!(
            query_reply.result,
            Some(get_query_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(query_reply.schemas.len(), 3);
    }

    #[test]
    fn get_empty_schemas() {
        let mut service = get_service(
            vec!["event", "other"],
            vec!["raw"],
            vec!["query", "other"],
            vec!["raw"],
        );
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["raw".to_string()],
        });
        assert_eq!(
            event_reply.result,
            Some(get_event_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(event_reply.schemas.len(), 1);
        assert_eq!(event_reply.schemas.keys().next().unwrap(), "raw");
        assert_eq!(event_reply.schemas.values().next().unwrap(), "");

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["raw".to_string()],
        });
        assert_eq!(
            query_reply.result,
            Some(get_query_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(query_reply.schemas.len(), 1);
        assert_eq!(query_reply.schemas.keys().next().unwrap(), "raw");
        assert_eq!(query_reply.schemas.values().next().unwrap().input, "");
        assert_eq!(query_reply.schemas.values().next().unwrap().output, "");
    }

    #[test]
    fn get_missing_schemas() {
        let mut service = get_service(
            vec!["event", "other"],
            vec![],
            vec!["query", "other"],
            vec![],
        );
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["main".to_string()],
        });
        assert!(matches!(
            event_reply.result,
            Some(get_event_source_schemas_reply::Result::Error(_))
        ));
        assert!(event_reply.schemas.is_empty());

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["main".to_string()],
        });
        assert!(matches!(
            query_reply.result,
            Some(get_query_source_schemas_reply::Result::Error(_))
        ));
        assert!(query_reply.schemas.is_empty());
    }
}
