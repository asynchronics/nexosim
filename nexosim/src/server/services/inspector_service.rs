use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::registry::{EventSinkRegistry, EventSourceRegistry, QuerySourceRegistry};

use super::super::codegen::simulation::*;
use super::{map_registry_error, simulation_not_started_error};

/// Protobuf-based simulation inspector.
///
/// An `InspectorService` provides information about available endpoints and
/// types of the messages passed.
pub(crate) enum InspectorService {
    NotStarted,
    Started {
        event_sink_registry: Arc<Mutex<EventSinkRegistry>>,
        event_source_registry: Arc<EventSourceRegistry>,
        query_source_registry: Arc<QuerySourceRegistry>,
    },
}

impl InspectorService {
    /// Returns a list of names of all the registered event sources.
    pub(crate) fn list_event_sources(&self, _: ListEventSourcesRequest) -> ListEventSourcesReply {
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

    /// Retrieves the input event schemas for the specified event sources.
    ///
    /// If the `source_names` field is left empty, it returns schemas for all
    /// the registered sources. Sources added with
    /// [`EndpointRegistry::add_event_source_raw`](crate::registry::EndpointRegistry::add_query_source_raw)
    /// would provide an empty string as their schema.
    pub(crate) fn get_event_source_schemas(
        &self,
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

    /// Returns a list of names of all the registered query sources.
    pub(crate) fn list_query_sources(&self, _: ListQuerySourcesRequest) -> ListQuerySourcesReply {
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

    /// Retrieves the input and output query schemas for the specified query
    /// sources.
    ///
    /// If the `source_names` field is left empty, it returns schemas for all
    /// the registered sources. Sources added with
    /// [`EndpointRegistry::add_query_source_raw`](crate::registry::EndpointRegistry::add_query_source_raw)
    /// would provide an empty string as their schema.
    pub(crate) fn get_query_source_schemas(
        &self,
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
                                    request: schema.0,
                                    reply: schema.1,
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
                                    request: schema.0,
                                    reply: schema.1,
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

    /// Returns a list of names of all the registered event sinks.
    pub(crate) fn list_event_sinks(&self, _: ListEventSinksRequest) -> ListEventSinksReply {
        match self {
            Self::Started {
                event_sink_registry,
                ..
            } => ListEventSinksReply {
                sink_names: event_sink_registry
                    .lock()
                    .unwrap()
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

    /// Returns the schemas of the specified event sinks.
    ///
    /// If `sink_names` field is empty, it returns schemas for all the
    /// the registered sinks. Sinks added with
    /// [`EndpointRegistry::add_event_sink_raw`](crate::registry::EndpointRegistry::add_event_sink_raw)
    /// would provide an empty string as their schema.
    pub(crate) fn get_event_sink_schemas(
        &self,
        request: GetEventSinkSchemasRequest,
    ) -> GetEventSinkSchemasReply {
        match self {
            Self::Started {
                event_sink_registry,
                ..
            } => {
                let event_sink_registry = event_sink_registry.lock().unwrap();
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

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use crate::ports::{EventSinkReader, EventSource, QuerySource};

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
        let mut event_source_registry = EventSourceRegistry::default();
        for source in params.event_sources {
            event_source_registry
                .add::<()>(EventSource::new(), source)
                .unwrap();
        }
        for source in params.raw_event_sources {
            event_source_registry
                .add_raw::<()>(EventSource::new(), source)
                .unwrap();
        }

        let mut query_source_registry = QuerySourceRegistry::default();
        for source in params.query_sources {
            query_source_registry
                .add::<(), ()>(QuerySource::new(), source)
                .unwrap();
        }
        for source in params.raw_query_sources {
            query_source_registry
                .add_raw::<(), ()>(QuerySource::new(), source)
                .unwrap();
        }

        let mut event_sink_registry = EventSinkRegistry::default();
        for sink in params.event_sinks {
            event_sink_registry.add(DummySink::new(()), sink).unwrap();
        }
        for sink in params.raw_event_sinks {
            event_sink_registry
                .add_raw(DummySink::new(()), sink)
                .unwrap();
        }

        InspectorService::Started {
            event_sink_registry: Arc::new(Mutex::new(event_sink_registry)),
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

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["sink".to_string()],
        });
        assert_eq!(
            sink_reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(sink_reply.schemas.len(), 1);
        assert_eq!(sink_reply.schemas.keys().next().unwrap(), "sink");
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
        assert_eq!(
            event_reply.result,
            Some(get_event_source_schemas_reply::Result::Empty(()))
        );
        assert_eq!(event_reply.schemas.len(), 2);
        let event_keys = event_reply.schemas.into_keys().collect::<HashSet<String>>();
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
        let query_keys = query_reply.schemas.into_keys().collect::<HashSet<String>>();
        assert!(query_keys.contains("query"));
        assert!(query_keys.contains("secondary"));

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["sink".to_string(), "secondary".to_string()],
        });
        assert_eq!(
            sink_reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(sink_reply.schemas.len(), 2);
        let sink_keys = sink_reply.schemas.into_keys().collect::<HashSet<String>>();
        assert!(sink_keys.contains("sink"));
        assert!(sink_keys.contains("secondary"));
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

        let sink_reply =
            service.get_event_sink_schemas(GetEventSinkSchemasRequest { sink_names: vec![] });
        assert_eq!(
            sink_reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(sink_reply.schemas.len(), 3);
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
        assert_eq!(query_reply.schemas.values().next().unwrap().request, "");
        assert_eq!(query_reply.schemas.values().next().unwrap().reply, "");

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["raw".to_string()],
        });
        assert_eq!(
            sink_reply.result,
            Some(get_event_sink_schemas_reply::Result::Empty(()))
        );
        assert_eq!(sink_reply.schemas.len(), 1);
        assert_eq!(sink_reply.schemas.keys().next().unwrap(), "raw");
        assert_eq!(sink_reply.schemas.values().next().unwrap(), "");
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

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sink_names: vec!["main".to_string()],
        });
        assert!(matches!(
            sink_reply.result,
            Some(get_event_sink_schemas_reply::Result::Error(_))
        ));
        assert!(sink_reply.schemas.is_empty());
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
        assert_eq!(HashSet::from_iter(reply.source_names), expected);
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
        assert_eq!(HashSet::from_iter(reply.source_names), expected);
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
        assert_eq!(HashSet::from_iter(reply.sink_names), expected);
    }
}
