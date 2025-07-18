mod controller_service;
mod init_service;
mod inspector_service;
mod monitor_service;
mod scheduler_service;

use std::time::Duration;

use prost_types::Timestamp;
use tai_time::MonotonicTime;

use super::codegen::simulation::{Error, ErrorCode};
use crate::registry::RegistryError;
use crate::simulation::{ExecutionError, SchedulingError, SimulationError};

pub use init_service::{init_bench, restore_bench};

pub(crate) use controller_service::ControllerService;
pub(crate) use init_service::{InitResult, InitService};
pub(crate) use inspector_service::InspectorService;
pub(crate) use monitor_service::MonitorService;
pub(crate) use scheduler_service::SchedulerService;

/// Transforms an error code and a message into a Protobuf error.
fn to_error(code: ErrorCode, message: impl Into<String>) -> Error {
    Error {
        code: code as i32,
        message: message.into(),
    }
}

/// An error returned when a simulation was not started.
fn simulation_not_started_error() -> Error {
    to_error(
        ErrorCode::SimulationNotStarted,
        "the simulation was not started",
    )
}

/// Map an `ExecutionError` to a Protobuf error.
fn map_execution_error(error: ExecutionError) -> Error {
    let error_code = match error {
        ExecutionError::Deadlock(_) => ErrorCode::SimulationDeadlock,
        ExecutionError::MessageLoss(_) => ErrorCode::SimulationMessageLoss,
        ExecutionError::NoRecipient { .. } => ErrorCode::SimulationNoRecipient,
        ExecutionError::Panic { .. } => ErrorCode::SimulationPanic,
        ExecutionError::Timeout => ErrorCode::SimulationTimeout,
        ExecutionError::OutOfSync(_) => ErrorCode::SimulationOutOfSync,
        ExecutionError::BadQuery => ErrorCode::SimulationBadQuery,
        ExecutionError::Halted => ErrorCode::SimulationHalted,
        ExecutionError::Terminated => ErrorCode::SimulationTerminated,
        ExecutionError::InvalidDeadline(_) => ErrorCode::InvalidDeadline,
        ExecutionError::InvalidEvent(_) => ErrorCode::SourceNotFound,
        ExecutionError::SaveError(_) => ErrorCode::SaveError,
        ExecutionError::RestoreError(_) => ErrorCode::RestoreError,
    };

    let error_message = error.to_string();

    to_error(error_code, error_message)
}

/// Map a `SchedulingError` to a Protobuf error.
fn map_scheduling_error(error: SchedulingError) -> Error {
    let error_code = match error {
        SchedulingError::InvalidScheduledTime => ErrorCode::InvalidDeadline,
        SchedulingError::NullRepetitionPeriod => ErrorCode::InvalidPeriod,
    };

    let error_message = error.to_string();

    to_error(error_code, error_message)
}

/// Map a `SimulationError` to a Protobuf error.
fn map_simulation_error(error: SimulationError) -> Error {
    match error {
        SimulationError::ExecutionError(e) => map_execution_error(e),
        SimulationError::SchedulingError(e) => map_scheduling_error(e),
    }
}

fn map_registry_error(error: RegistryError) -> Error {
    let error_code = match error {
        RegistryError::SourceNotFound(_) => ErrorCode::SourceNotFound,
        RegistryError::SinkNotFound(_) => ErrorCode::SinkNotFound,
        RegistryError::Unregistered => ErrorCode::Unregistered,
        RegistryError::InvalidType(_) => ErrorCode::InvalidType,
        RegistryError::DeserializationError(_) => ErrorCode::DeserializationError,
    };
    let error_message = error.to_string();

    to_error(error_code, error_message)
}

/// Attempts a cast from a `MonotonicTime` to a protobuf `Timestamp`.
///
/// This will fail if the time is outside the protobuf-specified range for
/// timestamps (0001-01-01 00:00:00 to 9999-12-31 23:59:59).
pub(crate) fn monotonic_to_timestamp(monotonic_time: MonotonicTime) -> Option<Timestamp> {
    // Unix timestamp for 0001-01-01 00:00:00, the minimum accepted by
    // protobuf's specification for the `Timestamp` type.
    const MIN_SECS: i64 = -62135596800;
    // Unix timestamp for 9999-12-31 23:59:59, the maximum accepted by
    // protobuf's specification for the `Timestamp` type.
    const MAX_SECS: i64 = 253402300799;

    let secs = monotonic_time.as_secs();
    if !(MIN_SECS..=MAX_SECS).contains(&secs) {
        return None;
    }

    Some(Timestamp {
        seconds: secs,
        nanos: monotonic_time.subsec_nanos() as i32,
    })
}

/// Attempts a cast from a protobuf `Timestamp` to a `MonotonicTime`.
///
/// This should never fail provided that the `Timestamp` complies with the
/// protobuf specification. It can only fail if the nanosecond part is negative
/// or greater than 999'999'999.
pub(crate) fn timestamp_to_monotonic(timestamp: Timestamp) -> Option<MonotonicTime> {
    let nanos: u32 = timestamp.nanos.try_into().ok()?;

    MonotonicTime::new(timestamp.seconds, nanos)
}

/// Attempts a cast from a protobuf `Duration` to a `std::time::Duration`.
///
/// If the `Duration` complies with the protobuf specification, this can only
/// fail if the duration is negative.
pub(crate) fn to_positive_duration(duration: prost_types::Duration) -> Option<Duration> {
    if duration.seconds < 0 || duration.nanos < 0 {
        return None;
    }

    Some(Duration::new(
        duration.seconds as u64,
        duration.nanos as u32,
    ))
}

/// Attempts a cast from a protobuf `Duration` to a strictly positive
/// `std::time::Duration`.
///
/// If the `Duration` complies with the protobuf specification, this can only
/// fail if the duration is negative or null.
pub(crate) fn to_strictly_positive_duration(duration: prost_types::Duration) -> Option<Duration> {
    if duration.seconds < 0 || duration.nanos < 0 || (duration.seconds == 0 && duration.nanos == 0)
    {
        return None;
    }

    Some(Duration::new(
        duration.seconds as u64,
        duration.nanos as u32,
    ))
}

#[cfg(test)]
macro_rules! assert_reply_err {
    ($reply: ident, $namespace: ident, $code: expr) => {
        let Some($namespace::Result::Error(Error { code, .. })) = $reply.result else {
            panic!("Invalid reply!")
        };
        assert_eq!(code, $code as i32);
    };
}
#[cfg(test)]
pub(crate) use assert_reply_err;
