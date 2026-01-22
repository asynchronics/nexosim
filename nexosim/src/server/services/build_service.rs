use std::any::Any;
use std::error;
use std::panic::{self, AssertUnwindSafe};

use ciborium;
use serde::de::DeserializeOwned;

use crate::endpoints::Endpoints;
use crate::server::services::from_bench_error;
use crate::simulation::{BenchError, SimInit, Simulation};

use super::super::codegen::simulation::*;
use super::{from_simulation_error, timestamp_to_monotonic, to_error};

type DeserializationError = ciborium::de::Error<std::io::Error>;
type SimGen = Box<
    dyn FnMut(&[u8]) -> Result<Result<SimInit, Box<dyn error::Error>>, DeserializationError>
        + Send
        + 'static,
>;

/// Protobuf-based simulation initializer.
///
/// A `BuildService` creates a new simulation bench based on a serialized
/// initialization configuration.
pub(crate) struct BuildService {
    sim_gen: SimGen,
    bench: Option<SimInit>,
}

impl BuildService {
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
            bench: None,
        }
    }

    /// Builds the simulation bench based on the specified configuration.
    pub(crate) fn build(&mut self, request: BuildRequest) -> Result<(), Error> {
        panic::catch_unwind(AssertUnwindSafe(|| {
            (self.sim_gen)(&request.cfg)
                .map_err(from_config_deserialization_error)
                .and_then(|bench_result| bench_result.map_err(from_general_bench_error))
        }))
        .map_err(from_panic)
        .and_then(|reply| reply)
        .map(|bench| self.bench = Some(bench))
    }

    /// Initializes the simulation.
    pub(crate) fn init(&mut self, request: InitRequest) -> Result<(Simulation, Endpoints), Error> {
        let Some(mut bench) = self.bench.take() else {
            return Err(bench_not_built_error());
        };

        let Some(start_time) = request.time.and_then(timestamp_to_monotonic) else {
            return Err(to_error(
                ErrorCode::MissingArgument,
                "simulation start time not provided",
            ));
        };

        let endpoints = bench.take_endpoints();
        bench
            .init(start_time)
            .map_err(from_simulation_error)
            .map(|simulation| (simulation, endpoints))
    }

    /// Restore the simulation from a serialized state.
    pub(crate) fn restore(
        &mut self,
        request: RestoreRequest,
    ) -> Result<(Simulation, Endpoints), Error> {
        let Some(mut bench) = self.bench.take() else {
            return Err(bench_not_built_error());
        };

        let endpoints = bench.take_endpoints();
        bench
            .restore(&request.state[..])
            .map_err(from_simulation_error)
            .map(|simulation| (simulation, endpoints))
    }
}

fn bench_not_built_error() -> Error {
    to_error(
        ErrorCode::BenchNotBuilt,
        "the simulation bench needs to be built first",
    )
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
            ErrorCode::BenchError,
            format!("simulation bench building has failed with the following error: {error}",),
        ),
    }
}
