use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use futures_channel::mpsc;
use futures_core::Stream;
use futures_util::stream::StreamExt;
use pin_project::pin_project;
use serde::Serialize;

use crate::model::Message;
use crate::simulation::{DuplicateEventSinkError, SimInit};

use super::{EventSinkReader, EventSinkWriter, SinkState};

/// Creates an event queue with an unbounded size.
///
/// This function returns an [`EventQueueReader`] and an [`EventQueueWriter`]
/// which implement [`EventSinkReader`] and [`EventSinkWriter`], respectively.
pub fn event_queue<T: Send>(state: SinkState) -> (EventQueueWriter<T>, EventQueueReader<T>) {
    let (sender, receiver) = mpsc::unbounded();
    let is_enabled = Arc::new(AtomicBool::new(state == SinkState::Enabled));

    let reader = EventQueueReader {
        receiver,
        is_enabled: is_enabled.clone(),
    };
    let writer = EventQueueWriter { sender, is_enabled };

    (writer, reader)
}

/// Creates an event queue with an unbounded size, adding it as a simulation
/// endpoint sink.
///
/// This is a convenience function and is equivalent to calling [`event_queue`]
/// and immediately registering the reader as an endpoint with
/// [`SimInit::bind_event_sink`].
pub fn event_queue_endpoint<T: Message + Serialize + Send + 'static>(
    sim_init: &mut SimInit,
    state: SinkState,
    name: impl Into<String>,
) -> Result<EventQueueWriter<T>, DuplicateEventSinkError> {
    let (writer, reader) = event_queue(state);

    sim_init.bind_event_sink(reader, name).map(|()| writer)
}

/// Creates an event queue with an unbounded size, adding it as a simulation
/// endpoint sink without requiring a [`Message`] implementation for its item
/// type.
///
/// This is a convenience function and is equivalent to calling [`event_queue`]
/// and immediately registering the reader as an endpoint with
/// [`SimInit::bind_event_sink_raw`].
pub fn event_queue_endpoint_raw<T: Serialize + Send + 'static>(
    sim_init: &mut SimInit,
    state: SinkState,
    name: impl Into<String>,
) -> Result<EventQueueWriter<T>, DuplicateEventSinkError> {
    let (writer, reader) = event_queue(state);

    sim_init.bind_event_sink_raw(reader, name).map(|()| writer)
}

/// The unique consumer handle of an event queue.
///
/// Implements [`EventSinkReader`].
#[pin_project]
pub struct EventQueueReader<T: Send> {
    is_enabled: Arc<AtomicBool>,
    #[pin]
    receiver: mpsc::UnboundedReceiver<T>,
}

impl<T: Send> Stream for EventQueueReader<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        this.receiver.poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.receiver.size_hint()
    }
}

impl<T: Send + 'static> EventSinkReader<T> for EventQueueReader<T> {
    fn enable(&mut self) {
        self.is_enabled.store(true, Ordering::Relaxed);
    }

    fn disable(&mut self) {
        self.is_enabled.store(false, Ordering::Relaxed);
    }

    fn try_read(&mut self) -> Option<T> {
        self.receiver.try_next().ok().and_then(|event| event)
    }

    fn read(&mut self) -> Option<T> {
        pollster::block_on(self.receiver.next())
    }
}

impl<T: Send> fmt::Debug for EventQueueReader<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventQueueReader")
            .field("is_enabled", &self.is_enabled.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// A cloneable producer handle of an event queue.
pub struct EventQueueWriter<T: Send> {
    is_enabled: Arc<AtomicBool>,
    sender: mpsc::UnboundedSender<T>,
}

impl<T: Send + 'static> EventSinkWriter<T> for EventQueueWriter<T> {
    /// Pushes an event onto the queue.
    fn write(&self, event: T) {
        if !self.is_enabled.load(Ordering::Relaxed) {
            return;
        }
        // Ignore sending failure.
        let _ = self.sender.unbounded_send(event);
    }
}

impl<T: Send> Clone for EventQueueWriter<T> {
    fn clone(&self) -> Self {
        Self {
            is_enabled: self.is_enabled.clone(),
            sender: self.sender.clone(),
        }
    }
}

impl<T: Send> fmt::Debug for EventQueueWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventQueueWriter")
            .field("is_enabled", &self.is_enabled.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}
