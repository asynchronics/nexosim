use std::fmt;

use futures_core::stream::Stream;

pub(crate) mod event_queue;

/// A writer handle to an event sink.
pub trait EventSinkWriter<T>: Clone + Send + Sync + 'static {
    /// Writes a value to the associated sink.
    fn write(&self, event: T);
}

/// An iterator over collected events with the ability to pause and resume event
/// collection. Accessing the next event can be blocking or non-blocking.
pub trait EventSinkReader<T>: Stream<Item = T> + Unpin {
    /// Starts or resumes the collection of new events.
    fn enable(&mut self);

    /// Pauses the collection of new events.
    ///
    /// Events that were collected in the queue prior to this call remain
    /// available.
    fn disable(&mut self);

    /// Returns the next event, if any.
    fn try_read(&mut self) -> Option<T>;

    /// Returns the next event, blocking the thread until the event becomes
    /// available if necessary.
    ///
    /// Returns `None` if all writers were dropped.
    fn read(&mut self) -> Option<T>;
}

/// The state of a sink.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SinkState {
    /// The sink accepts new events.
    Enabled,
    /// The sink ignores new events.
    Disabled,
}

impl fmt::Display for SinkState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SinkState::Enabled => write!(f, "enabled"),
            SinkState::Disabled => write!(f, "disabled"),
        }
    }
}
