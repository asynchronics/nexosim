use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

use ciborium;
use serde::de::DeserializeOwned;

use crate::registry::EndpointRegistry;
use crate::simulation::{ExecutionError, SimInit, Simulation, SimulationError};
use crate::time::MonotonicTime;

use super::{map_simulation_error, timestamp_to_monotonic, to_error};

use super::super::codegen::simulation::*;

type DeserializationError = ciborium::de::Error<std::io::Error>;
type SimGen = Box<dyn FnMut(&[u8]) -> Result<SimInit, DeserializationError> + Send + 'static>;

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
        F: FnMut(I) -> SimInit + Send + 'static,
        I: DeserializeOwned,
    {
        // Wrap `sim_gen` so it accepts a serialized init configuration.
        let sim_gen = move |serialized_cfg: &[u8]| -> Result<SimInit, DeserializationError> {
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
    ) -> (InitReply, Option<(Simulation, EndpointRegistry, Vec<u8>)>) {
        let Some(start_time) = request.time.and_then(timestamp_to_monotonic) else {
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

        let reply = panic::catch_unwind(AssertUnwindSafe(|| {
            (self.sim_gen)(&request.cfg).map(|sim_init| sim_init.init(start_time))
        }))
        .map_err(map_panic)
        .and_then(map_init_error);

        let (reply, bench) = match reply {
            Ok((simulation, registry)) => (
                init_reply::Result::Empty(()),
                Some((simulation, registry, request.cfg)),
            ),
            Err(e) => (init_reply::Result::Error(e), None),
        };

        (
            InitReply {
                result: Some(reply),
            },
            bench,
        )
    }

    /// Restore the simulation from a serialized state.
    pub(crate) fn restore(
        &mut self,
        request: RestoreRequest,
    ) -> (
        RestoreReply,
        Option<(Simulation, EndpointRegistry, Vec<u8>)>,
    ) {
        let Ok(Some(stored_cfg)) = Simulation::restore_cfg(&request.state[..]) else {
            return (
                RestoreReply {
                    result: Some(restore_reply::Result::Error(to_error(
                        ErrorCode::InvalidMessage,
                        "simulation state cannot be deserialized",
                    ))),
                },
                None,
            );
        };

        let cfg = match request.cfg {
            Some(cfg) => cfg,
            _ => stored_cfg,
        };

        let reply = panic::catch_unwind(AssertUnwindSafe(|| {
            (self.sim_gen)(&cfg).map(|sim_init| sim_init.restore(&request.state[..]))
        }))
        .map_err(map_panic)
        .and_then(map_init_error);

        let (reply, bench) = match reply {
            Ok((simulation, registry)) => (
                restore_reply::Result::Empty(()),
                Some((simulation, registry, cfg)),
            ),
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

fn map_panic(payload: Box<dyn Any + Send>) -> Error {
    let panic_msg: Option<&str> = if let Some(s) = payload.downcast_ref::<&str>() {
        Some(s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        Some(s)
    } else {
        None
    };

    let error_msg = if let Some(panic_msg) = panic_msg {
        format!("the simulation initializer has panicked with the message `{panic_msg}`",)
    } else {
        String::from("the simulation initializer has panicked")
    };

    to_error(ErrorCode::InitializerPanic, error_msg)
}

fn map_init_error(
    payload: Result<Result<(Simulation, EndpointRegistry), SimulationError>, DeserializationError>,
) -> Result<(Simulation, EndpointRegistry), Error> {
    payload
        .map_err(|e| {
            to_error(
                ErrorCode::InvalidMessage,
                format!("the initializer configuration could not be deserialized: {e}",),
            )
        })
        .and_then(|init_result| init_result.map_err(map_simulation_error))
}

/// Allows running a server targeted simulation directly from Rust. (e.g.
/// for debugging purposes)
pub fn init_bench<F, I>(
    mut sim_gen: F,
    cfg: I,
    start_time: MonotonicTime,
) -> Result<(Simulation, EndpointRegistry), SimulationError>
where
    F: FnMut(I) -> SimInit + Send + 'static,
    I: DeserializeOwned,
{
    sim_gen(cfg).init(start_time)
}

/// Allows restoring a previously saved server simulation and continuing it's
/// execution directly from Rust.
///
/// It is possible to override the initial configuration that the simulation
/// has been started with. The override should not modify bench nor model
/// layout, otherwise unexpected side effects might happen.
/// If no additional configuration is provided, simulation will be restored with
/// it's initial config value.
pub fn restore_bench<F, I>(
    mut sim_gen: F,
    state: &[u8],
    cfg: Option<I>,
) -> Result<(Simulation, EndpointRegistry), SimulationError>
where
    F: FnMut(I) -> SimInit + Send + 'static,
    I: DeserializeOwned,
{
    let cfg = match cfg {
        Some(a) => a,
        None => {
            let serialized_cfg = Simulation::restore_cfg(state)?.ok_or(
                ExecutionError::RestoreError("Bench config not found".to_string()),
            )?;
            ciborium::from_reader(&serialized_cfg[..]).unwrap()
        }
    };

    sim_gen(cfg).restore(state)
}
