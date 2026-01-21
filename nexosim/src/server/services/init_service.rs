use ciborium;
use serde::de::DeserializeOwned;

use crate::endpoints::Endpoints;
use crate::simulation::{ExecutionError, RestoreError, SimInit, Simulation, SimulationError};
use crate::time::MonotonicTime;

use super::super::codegen::simulation::*;
use super::{from_simulation_error, simulation_not_built_error, timestamp_to_monotonic, to_error};

/// Protobuf-based simulation initializer.
///
/// An `InitService` creates a new simulation from a simulation bench.
#[derive(Default)]
pub(crate) struct InitService(Option<SimInit>);

impl InitService {
    /// Builds an `InitService` that contains a readily-built simulation bench.
    pub(crate) fn ready(bench: SimInit) -> Self {
        Self(Some(bench))
    }

    /// Initializes the simulation.
    pub(crate) fn init(&mut self, request: InitRequest) -> Result<(Simulation, Endpoints), Error> {
        let Some(mut bench) = self.0.take() else {
            return Err(simulation_not_built_error());
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
        let Some(mut bench) = self.0.take() else {
            return Err(simulation_not_built_error());
        };

        // FIXME
        //let Ok(Some(stored_cfg)) = Simulation::restore_cfg(&request.state[..]) else {
        //    return Err(to_error(
        //        ErrorCode::InvalidMessage,
        //        "the simulation state cannot be deserialized",
        //    ));
        //};

        let endpoints = bench.take_endpoints();
        bench
            .restore(&request.state[..])
            .map_err(from_simulation_error)
            .map(|simulation| (simulation, endpoints))
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
