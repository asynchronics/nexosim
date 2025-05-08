use std::panic::{self, AssertUnwindSafe};

use ciborium;
use serde::de::DeserializeOwned;

use crate::registry::EndpointRegistry;
use crate::simulation::{SimInit, Simulation, SimulationError};

use super::{map_simulation_error, timestamp_to_monotonic, to_error};

use super::super::codegen::simulation::*;

type InitResult = Result<(SimInit, EndpointRegistry), SimulationError>;
type DeserializationError = ciborium::de::Error<std::io::Error>;
type SimGen = Box<dyn FnMut(&[u8]) -> Result<InitResult, DeserializationError> + Send + 'static>;

/// Protobuf-based simulation initializer.
///
/// An `InitService` creates a new simulation bench based on a serialized
/// initialization configuration.
pub(crate) struct InitService {
    sim_gen: SimGen,
}

impl InitService {
    /// Creates a new `InitService`.
    ///
    /// The argument is a closure that takes a CBOR-serialized initialization
    /// configuration and is called every time the simulation is (re)started by
    /// the remote client. It must create a new simulation complemented by a
    /// registry that exposes the public event and query interface.
    pub(crate) fn new<F, I>(mut sim_gen: F) -> Self
    where
        F: FnMut(I) -> Result<(SimInit, EndpointRegistry), SimulationError> + Send + 'static,
        I: DeserializeOwned,
    {
        // Wrap `sim_gen` so it accepts a serialized init configuration.
        let sim_gen = move |serialized_cfg: &[u8]| -> Result<InitResult, DeserializationError> {
            let cfg = ciborium::from_reader(serialized_cfg)?;

            Ok(sim_gen(cfg))
        };

        Self {
            sim_gen: Box::new(sim_gen),
        }
    }

    /// Initializes the simulation based on the specified configuration.
    pub(crate) fn init(
        &mut self,
        request: InitRequest,
    ) -> (InitReply, Option<(Simulation, EndpointRegistry)>) {
        let Some(start_time) = request.time.and_then(|t| timestamp_to_monotonic(t)) else {
            return (
                InitReply {
                    result: Some(init_reply::Result::Error(to_error(
                        ErrorCode::InvalidTime,
                        "simulation start time not provided",
                    ))),
                },
                None,
            );
        };

        let reply = panic::catch_unwind(AssertUnwindSafe(|| (self.sim_gen)(&request.cfg)))
            .map_err(|payload| {
                let panic_msg: Option<&str> = if let Some(s) = payload.downcast_ref::<&str>() {
                    Some(s)
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    Some(s)
                } else {
                    None
                };

                let error_msg = if let Some(panic_msg) = panic_msg {
                    format!(
                        "the simulation initializer has panicked with the message `{}`",
                        panic_msg
                    )
                } else {
                    String::from("the simulation initializer has panicked")
                };

                to_error(ErrorCode::InitializerPanic, error_msg)
            })
            .and_then(|res| {
                res.map_err(|e| {
                    to_error(
                        ErrorCode::InvalidMessage,
                        format!(
                            "the initializer configuration could not be deserialized: {}",
                            e
                        ),
                    )
                })
                .and_then(|init_result| init_result.map_err(map_simulation_error))
            });

        let (reply, bench) = match reply {
            Ok((mut sim_init, mut registry)) => {
                registry
                    .event_source_registry
                    .register_scheduler(&mut sim_init.scheduler_registry());
                match sim_init.init(start_time) {
                    Ok(simu) => (init_reply::Result::Empty(()), Some((simu, registry))),
                    Err(e) => (
                        init_reply::Result::Error(to_error(
                            ErrorCode::InitializerPanic,
                            &format!("the simulation initializer has panicked: {}", e),
                        )),
                        None,
                    ),
                }
            }
            Err(e) => (init_reply::Result::Error(e), None),
        };

        (
            InitReply {
                result: Some(reply),
            },
            bench,
        )
    }
}
