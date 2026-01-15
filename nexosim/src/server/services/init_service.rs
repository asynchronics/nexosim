use std::any::Any;
use std::error;
use std::panic::{self, AssertUnwindSafe};

use ciborium;
use serde::de::DeserializeOwned;

use crate::endpoints::Endpoints;
use crate::server::services::from_bench_error;
use crate::simulation::{
    BenchError, ExecutionError, RestoreError, SimInit, Simulation, SimulationError,
};
use crate::time::MonotonicTime;

use super::{from_simulation_error, timestamp_to_monotonic, to_error};

use super::super::codegen::simulation::*;

type DeserializationError = ciborium::de::Error<std::io::Error>;
type SimGen = Box<
    dyn FnMut(&[u8]) -> Result<Result<SimInit, Box<dyn error::Error>>, DeserializationError>
        + Send
        + 'static,
>;

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
        F: FnMut(I) -> Result<SimInit, Box<dyn error::Error>> + Send + 'static,
        I: DeserializeOwned,
    {
        // Wrap `sim_gen` so it accepts a serialized init configuration.
        let sim_gen = move |serialized_cfg: &[u8]| -> Result<
            Result<SimInit, Box<dyn error::Error>>,
            DeserializationError,
        > {
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
    ) -> (InitReply, Option<(Simulation, Endpoints, Vec<u8>)>) {
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
            (self.sim_gen)(&request.cfg)
                .map_err(from_config_deserialization_error)
                .and_then(|bench_result| bench_result.map_err(from_general_bench_error))
                .and_then(|mut bench| {
                    let endpoints = bench.take_endpoints();
                    bench
                        .init(start_time)
                        .map_err(from_simulation_error)
                        .map(|simulation| (simulation, endpoints))
                })
        }))
        .map_err(from_panic)
        .and_then(|reply| reply);

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
    ) -> (RestoreReply, Option<(Simulation, Endpoints, Vec<u8>)>) {
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
            (self.sim_gen)(&cfg)
                .map_err(from_config_deserialization_error)
                .and_then(|bench_result| bench_result.map_err(from_general_bench_error))
                .and_then(|mut bench| {
                    let endpoints = bench.take_endpoints();
                    bench
                        .restore(&request.state[..])
                        .map_err(from_simulation_error)
                        .map(|simulation| (simulation, endpoints))
                })
        }))
        .map_err(from_panic)
        .and_then(|reply| reply);

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

fn from_panic(payload: Box<dyn Any + Send>) -> Error {
    let panic_msg: Option<&str> = if let Some(s) = payload.downcast_ref::<&str>() {
        Some(s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        Some(s)
    } else {
        None
    };

    let error_msg = if let Some(panic_msg) = panic_msg {
        format!(
            "the simulation bench builder has panicked with the following message: `{panic_msg}`",
        )
    } else {
        String::from("the simulation bench builder has panicked")
    };

    to_error(ErrorCode::BenchPanic, error_msg)
}

fn from_config_deserialization_error(error: DeserializationError) -> Error {
    to_error(
        ErrorCode::InvalidMessage,
        format!("the simulation bench configuration could not be deserialized: {error}",),
    )
}

fn from_general_bench_error(error: Box<dyn error::Error>) -> Error {
    match error.downcast::<BenchError>() {
        Ok(bench_err) => from_bench_error(*bench_err),
        Err(error) => to_error(
            ErrorCode::UnknownBenchError,
            format!("simulation bench building has failed with the following error: {error}",),
        ),
    }
}

/// Allows running a server targeted simulation directly from Rust. (e.g.
/// for debugging purposes)
pub fn init_bench<F, I>(
    mut sim_gen: F,
    cfg: I,
    start_time: MonotonicTime,
) -> Result<(Simulation, Endpoints), SimulationError>
where
    F: FnMut(I) -> SimInit + Send + 'static,
    I: DeserializeOwned,
{
    let mut bench = sim_gen(cfg);
    let endpoints = bench.take_endpoints();

    bench
        .init(start_time)
        .map(|simulation| (simulation, endpoints))
}

/// Restores a previously saved simulation and returns its endpoints.
///
/// It is possible to provide a different bench generation function that the one
/// used originally and/or to provide an alternate bench configuration. It is
/// crucial, however, that this does not modify the set of models and their
/// paths. If no configuration is provided, the simulation is restored using the
/// previous initial configuration.
pub fn restore_bench<F, I>(
    mut sim_gen: F,
    state: &[u8],
    cfg: Option<I>,
) -> Result<(Simulation, Endpoints), SimulationError>
where
    F: FnMut(I) -> SimInit + Send + 'static,
    I: DeserializeOwned,
{
    let cfg = match cfg {
        Some(a) => a,
        None => {
            let serialized_cfg = Simulation::restore_cfg(state)?
                .ok_or(ExecutionError::RestoreError(RestoreError::ConfigMissing))?;
            ciborium::from_reader(&serialized_cfg[..]).unwrap()
        }
    };
    let mut bench = sim_gen(cfg);
    let endpoints = bench.take_endpoints();

    bench
        .restore(state)
        .map(|simulation| (simulation, endpoints))
}
