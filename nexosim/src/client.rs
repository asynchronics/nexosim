//! gRPC client for remote simulation control.

mod codegen;

pub use codegen::simulation::simulation_client::SimulationClient;
pub use codegen::simulation::*;
