use futures_core::stream::Stream;

pub(crate) mod event_queue;

/// A simulation endpoint that can receive events sent by model outputs.
///
/// An `EventSink` can be thought of as a self-standing input meant to
/// externally monitor the simulated system.
pub trait EventSink<T> {
    /// Writer handle to an event sink.
    type Writer: EventSinkWriter<T>;

    /// Returns the writer handle associated to this sink.
    fn writer(&self) -> Self::Writer;
}

/// A writer handle to an event sink.
pub trait EventSinkWriter<T>: Clone + Send + Sync + 'static {
    /// Writes a value to the associated sink.
    fn write(&self, event: T);
}

/// An iterator over collected events with the ability to pause and resume event
/// collection. Accessing the next event can be blocking or non-blocking.
pub trait EventSinkReader<T>: Stream<Item = T> + Unpin {
    /// Starts or resumes the collection of new events.
    fn open(&mut self);

    /// Pauses the collection of new events.
    ///
    /// Events that were previously in the stream remain available.
    fn close(&mut self);

    /// Returns the next event, if any.
    fn try_read(&mut self) -> Option<T>;

    /// Returns the next event, blocking the thread until the event becomes
    /// available if necessary.
    ///
    /// Returns `None` if all writers were dropped.
    fn read(&mut self) -> Option<T>;
}
