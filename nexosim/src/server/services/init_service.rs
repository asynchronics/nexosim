use std::panic::{self, AssertUnwindSafe};

use ciborium;
use serde::de::DeserializeOwned;

use crate::registry::EndpointRegistry;
use crate::simulation::{Scheduler, SimInit, Simulation, SimulationError};
use crate::util::serialization::get_serialization_config;

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
    ) -> (InitReply, Option<(Simulation, Scheduler, EndpointRegistry)>) {
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

        // TODO error handling
        let time = timestamp_to_monotonic(request.time.unwrap()).unwrap();

        let (reply, bench) = match reply {
            Ok((sim_init, mut registry)) => {
                sim_init
                    .with_queue(|q| registry.event_source_registry.register_scheduler_events(q));
                // TODO error handling
                let (simulation, scheduler) = sim_init.init(time).unwrap();
                (
                    init_reply::Result::Empty(()),
                    Some((simulation, scheduler, registry)),
                )
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

    pub(crate) fn restore(
        &mut self,
        request: RestoreRequest,
    ) -> (
        RestoreReply,
        Option<(Vec<u8>, Simulation, Scheduler, EndpointRegistry)>,
    ) {
        let (stored_cfg, state): (Vec<u8>, Vec<u8>) =
            bincode::serde::decode_from_slice(&request.state, get_serialization_config())
                .unwrap()
                .0;
        let cfg = match request.cfg {
            Some(restore_request::Cfg::Value(cfg)) => cfg,
            _ => stored_cfg,
        };

        let reply = panic::catch_unwind(AssertUnwindSafe(|| (self.sim_gen)(&cfg)))
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
            Ok((sim_init, mut registry)) => {
                sim_init
                    .with_queue(|q| registry.event_source_registry.register_scheduler_events(q));
                // TODO error handling
                let (simulation, scheduler) = sim_init.restore(&state).unwrap();
                (
                    restore_reply::Result::Empty(()),
                    Some((cfg, simulation, scheduler, registry)),
                )
            }
            Err(e) => (restore_reply::Result::Error(e), None),
        };

        (
            RestoreReply {
                result: Some(reply),
            },
            bench,
        )
    }
}
