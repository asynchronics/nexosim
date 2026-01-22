use std::sync::Arc;

use crate::endpoints::{EventSinkInfoRegistry, EventSourceRegistry, QuerySourceRegistry};
use crate::path::Path as NexosimPath;

use super::super::codegen::simulation::*;
use super::{from_endpoint_error, simulation_not_started_error};

/// Protobuf-based simulation inspector.
///
/// The `InspectorService` handles all requests that only involve immutable
/// access to endpoints.
pub(crate) enum InspectorService {
    Halted,
    Started {
        event_sink_info_registry: EventSinkInfoRegistry,
        event_source_registry: Arc<EventSourceRegistry>,
        query_source_registry: Arc<QuerySourceRegistry>,
    },
}

impl InspectorService {
    /// Returns a list of the paths of all registered event sources.
    pub(crate) fn list_event_sources(
        &self,
        _: ListEventSourcesRequest,
    ) -> Result<Vec<Path>, Error> {
        match self {
            Self::Started {
                event_source_registry,
                ..
            } => Ok(event_source_registry
                .list_sources()
                .map(|path| Path {
                    segments: path.to_vec_string(),
                })
                .collect()),
            Self::Halted => Err(simulation_not_started_error()),
        }
    }

    /// Retrieves the input event schemas for the specified event sources.
    ///
    /// If the `source_names` field is left empty, it returns schemas for all
    /// registered sources. Sources added with
    /// [`SimInit::bind_event_source_raw`](crate::simulation::SimInit::bind_event_source_raw)
    /// return an empty string as their schema.
    pub(crate) fn get_event_source_schemas(
        &self,
        request: GetEventSourceSchemasRequest,
    ) -> Result<Vec<EventSourceSchema>, Error> {
        let Self::Started {
            event_source_registry,
            ..
        } = self
        else {
            return Err(simulation_not_started_error());
        };

        let schemas: Result<Vec<_>, _> =
            if request.sources.is_empty() {
                // When no argument is provided, return all schemas.
                Ok(event_source_registry
                    .list_schemas()
                    .map(|(source, event)| EventSourceSchema {
                        source: Some(Path {
                            segments: source.to_vec_string(),
                        }),
                        event,
                    })
                    .collect())
            } else {
                request
                    .sources
                    .iter()
                    .map(|source| {
                        Ok(EventSourceSchema {
                            source: Some(source.clone()),
                            event: event_source_registry.get_source_schema(
                                &<NexosimPath as From<&[String]>>::from(&source.segments),
                            )?,
                        })
                    })
                    .collect()
            };

        schemas.map_err(from_endpoint_error)
    }

    /// Returns a list of names of all the registered query sources.
    pub(crate) fn list_query_sources(
        &self,
        _: ListQuerySourcesRequest,
    ) -> Result<Vec<Path>, Error> {
        match self {
            Self::Started {
                query_source_registry,
                ..
            } => Ok(query_source_registry
                .list_sources()
                .map(|path| Path {
                    segments: path.to_vec_string(),
                })
                .collect()),
            Self::Halted => Err(simulation_not_started_error()),
        }
    }

    /// Retrieves the input and output query schemas for the specified query
    /// sources.
    ///
    /// If the `source_names` field is left empty, it returns schemas for all
    /// the registered sources. Sources added with
    /// [`SimInit::bind_query_source_raw`](crate::simulation::SimInit::bind_query_source_raw)
    /// would provide an empty string as their schema.
    pub(crate) fn get_query_source_schemas(
        &self,
        request: GetQuerySourceSchemasRequest,
    ) -> Result<Vec<QuerySourceSchema>, Error> {
        let Self::Started {
            query_source_registry,
            ..
        } = self
        else {
            return Err(simulation_not_started_error());
        };

        let schema: Result<Vec<_>, _> = if request.sources.is_empty() {
            // When no argument is provided, return all schemas.
            Ok(query_source_registry
                .list_schemas()
                .map(|(source, request, reply)| QuerySourceSchema {
                    source: Some(Path {
                        segments: source.to_vec_string(),
                    }),
                    request,
                    reply,
                })
                .collect())
        } else {
            request
                .sources
                .iter()
                .map(|source| {
                    query_source_registry
                        .get_source_schema(&<NexosimPath as From<&[String]>>::from(
                            &source.segments,
                        ))
                        .map(|schema| QuerySourceSchema {
                            source: Some(source.clone()),
                            request: schema.0,
                            reply: schema.1,
                        })
                })
                .collect()
        };

        schema.map_err(from_endpoint_error)
    }

    /// Returns a list of names of all the registered event sinks.
    pub(crate) fn list_event_sinks(&self, _: ListEventSinksRequest) -> Result<Vec<Path>, Error> {
        match self {
            Self::Started {
                event_sink_info_registry,
                ..
            } => Ok(event_sink_info_registry
                .list_sinks()
                .map(|path| Path {
                    segments: path.to_vec_string(),
                })
                .collect()),

            Self::Halted => Err(simulation_not_started_error()),
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
    ) -> Result<Vec<EventSinkSchema>, Error> {
        let Self::Started {
            event_sink_info_registry,
            ..
        } = self
        else {
            return Err(simulation_not_started_error());
        };

        let schemas: Result<Vec<_>, _> = if request.sinks.is_empty() {
            Ok(event_sink_info_registry
                .list_schemas()
                .map(|(source, event)| EventSinkSchema {
                    sink: Some(Path {
                        segments: source.to_vec_string(),
                    }),
                    event,
                })
                .collect())
        } else {
            request
                .sinks
                .iter()
                .map(|sink| {
                    Ok(EventSinkSchema {
                        sink: Some(sink.clone()),
                        event: event_sink_info_registry.get_sink_schema(
                            &<NexosimPath as From<&[String]>>::from(&sink.segments),
                        )?,
                    })
                })
                .collect()
        };

        schemas.map_err(from_endpoint_error)
    }
}

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use crate::ports::{EventSource, QuerySource};
    use crate::simulation::SchedulerRegistry;

    #[derive(Default)]
    struct TestParams {
        event_sources: Vec<NexosimPath>,
        raw_event_sources: Vec<NexosimPath>,
        query_sources: Vec<NexosimPath>,
        raw_query_sources: Vec<NexosimPath>,
        event_sinks: Vec<NexosimPath>,
        raw_event_sinks: Vec<NexosimPath>,
    }

    fn get_service(params: TestParams) -> InspectorService {
        let mut scheduler_registry = SchedulerRegistry::default();
        let mut event_source_registry = EventSourceRegistry::default();
        for source in params.event_sources {
            event_source_registry
                .add::<()>(EventSource::new(), source, &mut scheduler_registry)
                .unwrap();
        }
        for source in params.raw_event_sources {
            event_source_registry
                .add_raw::<()>(EventSource::new(), source, &mut scheduler_registry)
                .unwrap();
        }

        let mut query_source_registry = QuerySourceRegistry::default();
        for source in params.query_sources {
            query_source_registry
                .add::<(), ()>(QuerySource::new(), source, &mut scheduler_registry)
                .unwrap();
        }
        for source in params.raw_query_sources {
            query_source_registry
                .add_raw::<(), ()>(QuerySource::new(), source, &mut scheduler_registry)
                .unwrap();
        }

        let mut event_sink_info_registry = EventSinkInfoRegistry::default();
        for sink in params.event_sinks {
            event_sink_info_registry.register::<()>(sink).unwrap();
        }
        for sink in params.raw_event_sinks {
            event_sink_info_registry.register_raw(sink).unwrap();
        }

        InspectorService::Started {
            event_sink_info_registry,
            event_source_registry: Arc::new(event_source_registry),
            query_source_registry: Arc::new(query_source_registry),
        }
    }

    #[test]
    fn get_single_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event".into(), "other".into()],
            query_sources: vec!["query".into(), "other".into()],
            event_sinks: vec!["sink".into(), "other".into()],
            ..Default::default()
        });

        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            sources: vec![Path {
                segments: vec!["event".to_string()],
            }],
        });
        assert!(event_reply.is_ok());
        let event_reply = event_reply.unwrap();
        assert_eq!(event_reply.len(), 1);
        assert_eq!(
            event_reply.first().map(|e| e.source.as_ref()),
            Some(Some(&Path {
                segments: vec!["event".to_string()]
            }))
        );

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            sources: vec![Path {
                segments: vec!["query".to_string()],
            }],
        });
        assert!(query_reply.is_ok());
        let query_reply = query_reply.unwrap();
        assert_eq!(query_reply.len(), 1);
        assert_eq!(
            query_reply.first().map(|e| e.source.as_ref()),
            Some(Some(&Path {
                segments: vec!["query".to_string()]
            }))
        );

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sinks: vec![Path {
                segments: vec!["sink".to_string()],
            }],
        });
        assert!(sink_reply.is_ok());
        let sink_reply = sink_reply.unwrap();
        assert_eq!(sink_reply.len(), 1);
        assert_eq!(
            sink_reply.first().map(|e| e.sink.as_ref()),
            Some(Some(&Path {
                segments: vec!["sink".to_string()]
            }))
        );
    }

    #[test]
    fn get_multiple_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event".into(), "secondary".into(), "other".into()],
            query_sources: vec!["query".into(), "secondary".into(), "other".into()],
            event_sinks: vec!["sink".into(), "secondary".into(), "other".into()],
            ..Default::default()
        });

        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            sources: vec![
                Path {
                    segments: vec!["event".to_string()],
                },
                Path {
                    segments: vec!["secondary".to_string()],
                },
            ],
        });
        assert!(event_reply.is_ok());
        let event_reply = event_reply.unwrap();
        assert_eq!(event_reply.len(), 2);
        assert!(event_reply.iter().any(|e| e.source.as_ref()
            == Some(&Path {
                segments: vec!["event".to_string()]
            })));
        assert!(event_reply.iter().any(|e| e.source.as_ref()
            == Some(&Path {
                segments: vec!["secondary".to_string()]
            })));

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            sources: vec![
                Path {
                    segments: vec!["query".to_string()],
                },
                Path {
                    segments: vec!["secondary".to_string()],
                },
            ],
        });
        assert!(query_reply.is_ok());
        let query_reply = query_reply.unwrap();
        assert_eq!(query_reply.len(), 2);
        assert!(query_reply.iter().any(|e| e.source.as_ref()
            == Some(&Path {
                segments: vec!["query".to_string()]
            })));
        assert!(query_reply.iter().any(|e| e.source.as_ref()
            == Some(&Path {
                segments: vec!["secondary".to_string()]
            })));

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sinks: vec![
                Path {
                    segments: vec!["sink".to_string()],
                },
                Path {
                    segments: vec!["secondary".to_string()],
                },
            ],
        });
        assert!(sink_reply.is_ok());
        let sink_reply = sink_reply.unwrap();
        assert_eq!(sink_reply.len(), 2);
        assert!(sink_reply.iter().any(|e| e.sink.as_ref()
            == Some(&Path {
                segments: vec!["sink".to_string()]
            })));
        assert!(query_reply.iter().any(|e| e.source.as_ref()
            == Some(&Path {
                segments: vec!["secondary".to_string()]
            })));
    }

    #[test]
    fn get_all_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event".into(), "other".into()],
            raw_event_sources: vec!["raw".into()],
            query_sources: vec!["query".into(), "other".into()],
            raw_query_sources: vec!["raw".into()],
            event_sinks: vec!["sink".into(), "secondary".into()],
            raw_event_sinks: vec!["raw".into()],
        });
        let event_reply =
            service.get_event_source_schemas(GetEventSourceSchemasRequest { sources: vec![] });
        assert!(event_reply.is_ok());
        assert_eq!(event_reply.unwrap().len(), 3);

        let query_reply =
            service.get_query_source_schemas(GetQuerySourceSchemasRequest { sources: vec![] });
        assert!(query_reply.is_ok());
        assert_eq!(query_reply.unwrap().len(), 3);

        let sink_reply =
            service.get_event_sink_schemas(GetEventSinkSchemasRequest { sinks: vec![] });
        assert!(sink_reply.is_ok());
        assert_eq!(sink_reply.unwrap().len(), 3);
    }

    #[test]
    fn get_empty_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event".into(), "other".into()],
            raw_event_sources: vec!["raw".into()],
            query_sources: vec!["query".into(), "other".into()],
            raw_query_sources: vec!["raw".into()],
            event_sinks: vec!["sink".into(), "other".into()],
            raw_event_sinks: vec!["raw".into()],
        });
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            sources: vec![Path {
                segments: vec!["raw".to_string()],
            }],
        });
        assert!(event_reply.is_ok());
        let event_reply = event_reply.unwrap();
        assert_eq!(event_reply.len(), 1);
        assert_eq!(
            event_reply.first().unwrap(),
            &EventSourceSchema {
                source: Some(Path {
                    segments: vec!["raw".to_string()]
                }),
                event: "".to_string(),
            }
        );

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            sources: vec![Path {
                segments: vec!["raw".to_string()],
            }],
        });
        assert!(query_reply.is_ok());
        let query_reply = query_reply.unwrap();
        assert_eq!(query_reply.len(), 1);
        assert_eq!(
            query_reply.first().unwrap(),
            &QuerySourceSchema {
                source: Some(Path {
                    segments: vec!["raw".to_string()]
                }),
                request: "".to_string(),
                reply: "".to_string(),
            }
        );

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sinks: vec![Path {
                segments: vec!["raw".to_string()],
            }],
        });
        assert!(sink_reply.is_ok());
        let sink_reply = sink_reply.unwrap();
        assert_eq!(sink_reply.len(), 1);
        assert_eq!(
            sink_reply.first().unwrap(),
            &EventSinkSchema {
                sink: Some(Path {
                    segments: vec!["raw".to_string()]
                }),
                event: "".to_string(),
            }
        );
    }

    #[test]
    fn get_missing_schemas() {
        let service = get_service(TestParams {
            event_sources: vec!["event".into(), "other".into()],
            query_sources: vec!["query".into(), "other".into()],
            event_sinks: vec!["sink".into(), "other".into()],
            ..Default::default()
        });
        let event_reply = service.get_event_source_schemas(GetEventSourceSchemasRequest {
            sources: vec![Path {
                segments: vec!["main".to_string()],
            }],
        });
        assert!(event_reply.is_err());

        let query_reply = service.get_query_source_schemas(GetQuerySourceSchemasRequest {
            sources: vec![Path {
                segments: vec!["main".to_string()],
            }],
        });
        assert!(query_reply.is_err());

        let sink_reply = service.get_event_sink_schemas(GetEventSinkSchemasRequest {
            sinks: vec![Path {
                segments: vec!["main".to_string()],
            }],
        });
        assert!(sink_reply.is_err());
    }

    #[test]
    fn list_event_sources() {
        let service = get_service(TestParams {
            event_sources: vec!["main".into(), ["other", "path"].into()],
            raw_event_sources: vec!["".into()],
            ..Default::default()
        });
        let reply = service.list_event_sources(ListEventSourcesRequest {});
        let expected: HashSet<Vec<String>> = HashSet::from_iter([
            vec!["main".to_string()],
            vec!["other".to_string(), "path".to_string()],
            vec!["".to_string()],
        ]);
        assert!(reply.is_ok());
        assert_eq!(
            HashSet::from_iter(reply.unwrap().into_iter().map(|e| e.segments)),
            expected
        );
    }

    #[test]
    fn list_query_sources() {
        let service = get_service(TestParams {
            query_sources: vec!["main".into(), ["other", "path"].into()],
            raw_query_sources: vec!["".into()],
            ..Default::default()
        });
        let reply = service.list_query_sources(ListQuerySourcesRequest {});
        let expected: HashSet<Vec<String>> = HashSet::from_iter([
            vec!["main".to_string()],
            vec!["other".to_string(), "path".to_string()],
            vec!["".to_string()],
        ]);
        assert!(reply.is_ok());
        assert_eq!(
            HashSet::from_iter(reply.unwrap().into_iter().map(|e| e.segments)),
            expected
        );
    }

    #[test]
    fn list_event_sinks() {
        let service = get_service(TestParams {
            event_sinks: vec!["main".into(), ["other", "path"].into()],
            raw_event_sinks: vec!["".into()],
            ..Default::default()
        });

        let reply = service.list_event_sinks(ListEventSinksRequest {});
        let expected: HashSet<Vec<String>> = HashSet::from_iter([
            vec!["main".to_string()],
            vec!["other".to_string(), "path".to_string()],
            vec!["".to_string()],
        ]);
        assert!(reply.is_ok());
        assert_eq!(
            HashSet::from_iter(reply.unwrap().into_iter().map(|e| e.segments)),
            expected
        );
    }
}
