use std::any::Any;
use std::error;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use ciborium;
use serde::de::DeserializeOwned;

use crate::endpoints::{
    EventSinkInfoRegistry, EventSinkRegistry, EventSourceRegistry, QuerySourceRegistry,
};
use crate::server::services::from_bench_error;
use crate::simulation::{BenchError, Injector, SimInit, Simulation};

use super::super::codegen::simulation::*;
use super::{bench_not_built_error, from_simulation_error, timestamp_to_monotonic, to_error};

type DeserializationError = ciborium::de::Error<std::io::Error>;
type SimGen = Box<
    dyn FnMut(&[u8]) -> Result<Result<SimInit, Box<dyn error::Error>>, DeserializationError>
        + Send
        + 'static,
>;

#[allow(clippy::large_enum_variant)]
enum BuildState {
    None,
    Built(
        SimInit,
        EventSinkRegistry,
        Arc<EventSourceRegistry>,
        Arc<QuerySourceRegistry>,
    ),
    Initialized,
}

/// Protobuf-based simulation initializer.
///
/// A `BuildService` creates a new simulation bench based on a serialized
/// initialization configuration.
pub(crate) struct BuildService {
    sim_gen: SimGen,
    state: BuildState,
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
            state: BuildState::None,
        }
    }

    /// Builds the simulation bench based on the specified configuration.
    pub(crate) fn build(
        &mut self,
        request: BuildRequest,
    ) -> Result<
        (
            EventSinkInfoRegistry,
            Arc<EventSourceRegistry>,
            Arc<QuerySourceRegistry>,
            Injector,
        ),
        Error,
    > {
        let BuildState::None = self.state else {
            return Err(to_error(
                ErrorCode::BenchAlreadyBuilt,
                "bench is already built",
            ));
        };

        panic::catch_unwind(AssertUnwindSafe(|| {
            (self.sim_gen)(&request.cfg)
                .map_err(from_config_deserialization_error)
                .and_then(|bench_result| bench_result.map_err(from_general_bench_error))
        }))
        .map_err(from_panic)
        .and_then(|reply| reply)
        .map(|mut bench| {
            let (
                event_sink_registry,
                event_sink_info_registry,
                event_source_registry,
                query_source_registry,
            ) = bench.take_endpoints().into_parts();

            let event_source_registry = Arc::new(event_source_registry);
            let query_source_registry = Arc::new(query_source_registry);
            let injector = bench.injector();

            self.state = BuildState::Built(
                bench,
                event_sink_registry,
                event_source_registry.clone(),
                query_source_registry.clone(),
            );

            (
                event_sink_info_registry,
                event_source_registry,
                query_source_registry,
                injector,
            )
        })
    }

    /// Initializes the simulation.
    pub(crate) fn init(
        &mut self,
        request: InitRequest,
    ) -> Result<
        (
            Simulation,
            EventSinkRegistry,
            Arc<EventSourceRegistry>,
            Arc<QuerySourceRegistry>,
        ),
        Error,
    > {
        let start_time = request
            .time
            .and_then(timestamp_to_monotonic)
            .ok_or_else(|| {
                to_error(
                    ErrorCode::MissingArgument,
                    "simulation start time not provided",
                )
            })?;

        // Check current state before swapping to `Initialized`.
        let BuildState::Built(_, _, _, _) = self.state else {
            return Err(bench_not_built_error());
        };

        // Method is executed under mutex so this should be infallible.
        let BuildState::Built(
            bench,
            event_sink_registry,
            event_source_registry,
            query_source_registry,
        ) = std::mem::replace(&mut self.state, BuildState::Initialized)
        else {
            unreachable!();
        };

        bench
            .init(start_time)
            .map_err(from_simulation_error)
            .map(|simulation| {
                (
                    simulation,
                    event_sink_registry,
                    event_source_registry,
                    query_source_registry,
                )
            })
    }

    /// Restore the simulation from a serialized state.
    pub(crate) fn restore(
        &mut self,
        request: RestoreRequest,
    ) -> Result<
        (
            Simulation,
            EventSinkRegistry,
            Arc<EventSourceRegistry>,
            Arc<QuerySourceRegistry>,
        ),
        Error,
    > {
        // Check current state before swapping to `Initialized`.
        let BuildState::Built(_, _, _, _) = self.state else {
            return Err(bench_not_built_error());
        };

        // Method is executed under mutex so this should be infallible.
        let BuildState::Built(
            bench,
            event_sink_registry,
            event_source_registry,
            query_source_registry,
        ) = std::mem::replace(&mut self.state, BuildState::Initialized)
        else {
            unreachable!();
        };

        bench
            .restore(&request.state[..])
            .map_err(from_simulation_error)
            .map(|simulation| {
                (
                    simulation,
                    event_sink_registry,
                    event_source_registry,
                    query_source_registry,
                )
            })
    }

    /// Sets state to `BuildState::None`.
    ///
    /// `None` variant is required for the `build` method to execute.
    pub(crate) fn reset_state(&mut self) {
        self.state = BuildState::None;
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
            ErrorCode::BenchError,
            format!("simulation bench building has failed with the following error: {error}",),
        ),
    }
}
