use std::collections::HashMap;
use std::sync::Arc;

use prost_types::Timestamp;

use crate::endpoints::{EventSinkInfoRegistry, EventSourceRegistry, QuerySourceRegistry};
use crate::simulation::Scheduler;

use super::super::codegen::simulation::*;
use super::{map_endpoint_error, monotonic_to_timestamp, simulation_halted_error, to_error};

/// Protobuf-based simulation inspector.
///
/// The `InspectorService` handles all requests that only involve immutable
/// resources.
pub(crate) enum InspectorService {
    Halted,
    Started {
        scheduler: Scheduler,
        event_sink_info_registry: EventSinkInfoRegistry,
        event_source_registry: Arc<EventSourceRegistry>,
        query_source_registry: Arc<QuerySourceRegistry>,
    },
}

impl InspectorService {
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

    /// Returns a list of names of all the registered event sources.
    pub(crate) fn list_event_sources(
        &self,
        _: ListEventSourcesRequest,
    ) -> Result<Vec<String>, Error> {
        match self {
            Self::Started {
                event_source_registry,
                ..
            } => Ok(event_source_registry
                .list_sources()
                .map(|a| a.to_string())
                .collect()),
            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Retrieves the input event schemas for the specified event sources.
    ///
    /// If the `source_names` field is left empty, it returns schemas for all
    /// registered sources. Sources added with
    /// [`SimInit::add_event_source_raw`](crate::simulation::SimInit::add_event_source_raw)
    /// return an empty string as their schema.
    pub(crate) fn get_event_source_schemas(
        &self,
        request: GetEventSourceSchemasRequest,
    ) -> Result<HashMap<String, String>, Error> {
        let Self::Started {
            event_source_registry,
            ..
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let schemas: Result<HashMap<_, _>, _> = if request.source_names.is_empty() {
            // When no argument is provided, return all schemas.
            event_source_registry
                .list_sources()
                .map(|source_name| {
                    Ok((
                        source_name.to_string(),
                        event_source_registry.get_source_schema(source_name)?,
                    ))
                })
                .collect()
        } else {
            request
                .source_names
                .iter()
                .map(|source_name| {
                    Ok((
                        source_name.to_string(),
                        event_source_registry.get_source_schema(source_name)?,
                    ))
                })
                .collect()
        };

        schemas.map_err(map_endpoint_error)
    }

    /// Returns a list of names of all the registered query sources.
    pub(crate) fn list_query_sources(
        &self,
        _: ListQuerySourcesRequest,
    ) -> Result<Vec<String>, Error> {
        match self {
            Self::Started {
                query_source_registry,
                ..
            } => Ok(query_source_registry
                .list_sources()
                .map(|source_name| source_name.to_string())
                .collect()),
            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Retrieves the input and output query schemas for the specified query
    /// sources.
    ///
    /// If the `source_names` field is left empty, it returns schemas for all
    /// the registered sources. Sources added with
    /// [`SimInit::add_query_source_raw`](crate::simulation::SimInit::add_query_source_raw)
    /// would provide an empty string as their schema.
    pub(crate) fn get_query_source_schemas(
        &self,
        request: GetQuerySourceSchemasRequest,
    ) -> Result<HashMap<String, QuerySchema>, Error> {
        let Self::Started {
            query_source_registry,
            ..
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let schema: Result<HashMap<_, _>, _> = if request.source_names.is_empty() {
            // When no argument is provided return all the schemas.
            query_source_registry
                .list_sources()
                .map(|source_name| {
                    query_source_registry
                        .get_source_schema(source_name)
                        .map(|schema| {
                            (
                                source_name.to_string(),
                                QuerySchema {
                                    request: schema.0,
                                    reply: schema.1,
                                },
                            )
                        })
                })
                .collect()
        } else {
            request
                .source_names
                .iter()
                .map(|source_name| {
                    query_source_registry
                        .get_source_schema(source_name)
                        .map(|schema| {
                            (
                                source_name.to_string(),
                                QuerySchema {
                                    request: schema.0,
                                    reply: schema.1,
                                },
                            )
                        })
                })
                .collect()
        };

        schema.map_err(map_endpoint_error)
    }

    /// Returns a list of names of all the registered event sinks.
    pub(crate) fn list_event_sinks(&self, _: ListEventSinksRequest) -> Result<Vec<String>, Error> {
        match self {
            Self::Started {
                event_sink_info_registry,
                ..
            } => Ok(event_sink_info_registry
                .list_all()
                .map(|a| a.to_string())
                .collect()),

            Self::Halted => Err(simulation_halted_error()),
        }
    }

    /// Returns the schemas of the specified event sinks.
    ///
    /// If `sink_names` field is empty, it returns schemas for all the the
    /// registered sinks. Sinks added with
    /// [`SimInit::bind_event_sink_raw`](crate::simulation::SimInit::bind_event_sink_raw)
    /// would provide an empty string as their schema.
    pub(crate) fn get_event_sink_schemas(
        &self,
        request: GetEventSinkSchemasRequest,
    ) -> Result<HashMap<String, String>, Error> {
        let Self::Started {
            event_sink_info_registry,
            ..
        } = self
        else {
            return Err(simulation_halted_error());
        };

        let schemas: Result<HashMap<_, _>, _> = if request.sink_names.is_empty() {
            event_sink_info_registry
                .list_all()
                .map(|sink_names| {
                    Ok((
                        sink_names.to_string(),
                        event_sink_info_registry.event_schema(sink_names)?,
                    ))
                })
                .collect()
        } else {
            request
                .sink_names
                .iter()
                .map(|sink_names| {
                    Ok((
                        sink_names.to_string(),
                        event_sink_info_registry.event_schema(sink_names)?,
                    ))
                })
                .collect()
        };

        schemas.map_err(map_endpoint_error)
    }
}

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use crate::ports::{EventSource, QuerySource};
    use crate::simulation::SchedulerRegistry;

    #[derive(Default)]
    struct TestParams<'a> {
        event_sources: Vec<&'a str>,
        raw_event_sources: Vec<&'a str>,
        query_sources: Vec<&'a str>,
        raw_query_sources: Vec<&'a str>,
        event_sinks: Vec<&'a str>,
        raw_event_sinks: Vec<&'a str>,
    }

    fn get_service(params: TestParams) -> InspectorService {
        let mut scheduler_registry = SchedulerRegistry::default();
        let mut event_source_registry = EventSourceRegistry::default();
        for source in params.event_sources {
            event_source_registry
                .add::<()>(
                    EventSource::new(),
                    source.to_string(),
                    &mut scheduler_registry,
                )
                .unwrap();
        }
        for source in params.raw_event_sources {
            event_source_registry
                .add_raw::<()>(
                    EventSource::new(),
                    source.to_string(),
                    &mut scheduler_registry,
                )
                .unwrap();
        }

        let mut query_source_registry = QuerySourceRegistry::default();
        for source in params.query_sources {
            query_source_registry
                .add::<(), ()>(
                    QuerySource::new(),
                    source.to_string(),
                    &mut scheduler_registry,
                )
                .unwrap();
        }
        for source in params.raw_query_sources {
            query_source_registry
                .add_raw::<(), ()>(
                    QuerySource::new(),
                    source.to_string(),
                    &mut scheduler_registry,
                )
                .unwrap();
        }

        let mut event_sink_info_registry = EventSinkInfoRegistry::default();
        for sink in params.event_sinks {
            event_sink_info_registry
                .register::<()>(sink.to_string())
                .unwrap();
        }
        for sink in params.raw_event_sinks {
            event_sink_info_registry
                .register_raw(sink.to_string())
                .unwrap();
        }

        InspectorService::Started {
            scheduler: Scheduler::dummy(),
            event_sink_info_registry,
            event_source_registry: Arc::new(event_source_registry),
            query_source_registry: Arc::new(query_source_registry),
        }
    }

    #[test]
    fn get_single_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event", "other"],
            query_sources: vec!["query", "other"],
            event_sinks: vec!["sink", "other"],
            ..Default::default()
        });

        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["event".to_string()],
        });
        assert!(event_reply.is_ok());
        let event_reply = event_reply.unwrap();
        assert_eq!(event_reply.len(), 1);
        assert_eq!(event_reply.keys().next(), Some(&"event".to_string()));

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["query".to_string()],
        });
        assert!(query_reply.is_ok());
        let query_reply = query_reply.unwrap();
        assert_eq!(query_reply.len(), 1);
        assert_eq!(query_reply.keys().next(), Some(&"query".to_string()));

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["sink".to_string()],
        });
        assert!(sink_reply.is_ok());
        let sink_reply = sink_reply.unwrap();
        assert_eq!(sink_reply.len(), 1);
        assert_eq!(sink_reply.keys().next(), Some(&"sink".to_string()));
    }

    #[test]
    fn get_multiple_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event", "secondary", "other"],
            query_sources: vec!["query", "secondary", "other"],
            event_sinks: vec!["sink", "secondary", "other"],
            ..Default::default()
        });

        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["event".to_string(), "secondary".to_string()],
        });
        assert!(event_reply.is_ok());
        let event_reply = event_reply.unwrap();
        assert_eq!(event_reply.len(), 2);
        assert!(event_reply.contains_key("event"));
        assert!(event_reply.contains_key("secondary"));

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["query".to_string(), "secondary".to_string()],
        });
        assert!(query_reply.is_ok());
        let query_reply = query_reply.unwrap();
        assert_eq!(query_reply.len(), 2);
        assert!(query_reply.contains_key("query"));
        assert!(query_reply.contains_key("secondary"));

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["sink".to_string(), "secondary".to_string()],
        });
        assert!(sink_reply.is_ok());
        let sink_reply = sink_reply.unwrap();
        assert_eq!(sink_reply.len(), 2);
        assert!(sink_reply.contains_key("sink"));
        assert!(sink_reply.contains_key("secondary"));
    }

    #[test]
    fn get_all_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event", "other"],
            raw_event_sources: vec!["raw"],
            query_sources: vec!["query", "other"],
            raw_query_sources: vec!["raw"],
            event_sinks: vec!["sink", "secondary"],
            raw_event_sinks: vec!["raw"],
        });
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec![],
        });
        assert!(event_reply.is_ok());
        assert_eq!(event_reply.unwrap().len(), 3);

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec![],
        });
        assert!(query_reply.is_ok());
        assert_eq!(query_reply.unwrap().len(), 3);

        let sink_reply =
            service.get_event_sink_schemas(GetEventSinkSchemasRequest { sink_names: vec![] });
        assert!(sink_reply.is_ok());
        assert_eq!(sink_reply.unwrap().len(), 3);
    }

    #[test]
    fn get_empty_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event", "other"],
            raw_event_sources: vec!["raw"],
            query_sources: vec!["query", "other"],
            raw_query_sources: vec!["raw"],
            event_sinks: vec!["sink", "other"],
            raw_event_sinks: vec!["raw"],
        });
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["raw".to_string()],
        });
        assert!(event_reply.is_ok());
        let event_reply = event_reply.unwrap();
        assert_eq!(event_reply.len(), 1);
        assert_eq!(event_reply.keys().next().unwrap(), "raw");
        assert_eq!(event_reply.values().next().unwrap(), "");

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["raw".to_string()],
        });
        assert!(query_reply.is_ok());
        let query_reply = query_reply.unwrap();
        assert_eq!(query_reply.len(), 1);
        assert_eq!(query_reply.keys().next().unwrap(), "raw");
        assert_eq!(query_reply.values().next().unwrap().request, "");
        assert_eq!(query_reply.values().next().unwrap().reply, "");

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["raw".to_string()],
        });
        assert!(sink_reply.is_ok());
        let sink_reply = sink_reply.unwrap();
        assert_eq!(sink_reply.len(), 1);
        assert_eq!(sink_reply.keys().next().unwrap(), "raw");
        assert_eq!(sink_reply.values().next().unwrap(), "");
    }

    #[test]
    fn get_missing_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event", "other"],
            query_sources: vec!["query", "other"],
            event_sinks: vec!["sink", "other"],
            ..Default::default()
        });
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            source_names: vec!["main".to_string()],
        });
        assert!(event_reply.is_err());

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            source_names: vec!["main".to_string()],
        });
        assert!(query_reply.is_err());

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["main".to_string()],
        });
        assert!(sink_reply.is_err());
    }

    #[test]
    fn list_event_sources() {
        let service = get_service(TestParams {
            event_sources: vec!["main", "other"],
            raw_event_sources: vec!["raw"],
            ..Default::default()
        });
        let reply = service.list_event_sources(ListEventSourcesRequest {});
        let expected: HashSet<String> =
            HashSet::from_iter(["main".to_string(), "other".to_string(), "raw".to_string()]);
        assert!(reply.is_ok());
        assert_eq!(HashSet::from_iter(reply.unwrap()), expected);
    }

    #[test]
    fn list_query_sources() {
        let service = get_service(TestParams {
            query_sources: vec!["main", "other"],
            raw_query_sources: vec!["raw"],
            ..Default::default()
        });
        let reply = service.list_query_sources(ListQuerySourcesRequest {});
        let expected: HashSet<String> =
            HashSet::from_iter(["main".to_string(), "other".to_string(), "raw".to_string()]);
        assert!(reply.is_ok());
        assert_eq!(HashSet::from_iter(reply.unwrap()), expected);
    }

    #[test]
    fn list_event_sinks() {
        let service = get_service(TestParams {
            event_sinks: vec!["main", "other"],
            raw_event_sinks: vec!["raw"],
            ..Default::default()
        });

        let reply = service.list_event_sinks(ListEventSinksRequest {});

        let expected: HashSet<String> =
            HashSet::from_iter(["main".to_string(), "other".to_string(), "raw".to_string()]);
        assert!(reply.is_ok());
        assert_eq!(HashSet::from_iter(reply.unwrap()), expected);
    }
}
