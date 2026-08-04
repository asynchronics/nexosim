//! gRPC client for remote simulation control.

mod codegen;

pub use codegen::simulation::simulation_client::SimulationClient;
pub use codegen::simulation::*;
use serde::Serialize;

type SerializationError = ciborium::ser::Error<std::io::Error>;

/// Encodes provided payload into CBOR format, as required by the simulation
/// server API.
pub fn encode_payload<T: Serialize>(value: &T) -> Result<Vec<u8>, SerializationError> {
    let mut buf = vec![];
    ciborium::into_writer(value, &mut buf)?;
    Ok(buf)
}
