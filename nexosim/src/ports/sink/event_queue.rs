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

use super::{EventSink, EventSinkReader, EventSinkWriter};

/// An event queue with an unbounded size.
///
/// Implements [`EventSink`].
///
/// Note that [`EventSinkReader`] is implemented by [`EventQueueReader`],
/// created with the [`EventQueue::into_reader`] method.
pub struct EventQueue<T: Send> {
    is_open: Arc<AtomicBool>,
    sender: mpsc::UnboundedSender<T>,
    receiver: mpsc::UnboundedReceiver<T>,
}

impl<T: Send> EventQueue<T> {
    /// Creates an open `EventQueue`.
    pub fn new_open() -> Self {
        Self::new_with_state(true)
    }

    /// Creates a closed `EventQueue`.
    pub fn new_closed() -> Self {
        Self::new_with_state(false)
    }

    /// Returns a consumer handle in the non-blocking mode.
    pub fn into_reader(self) -> EventQueueReader<T> {
        EventQueueReader {
            is_open: self.is_open,
            receiver: self.receiver,
        }
    }

    /// Creates a new `EventQueue` in the specified state.
    fn new_with_state(is_open: bool) -> Self {
        let (sender, receiver) = mpsc::unbounded();
        Self {
            is_open: Arc::new(AtomicBool::new(is_open)),
            sender,
            receiver,
        }
    }
}

impl<T: Send + 'static> EventSink<T> for EventQueue<T> {
    type Writer = EventQueueWriter<T>;

    fn writer(&self) -> Self::Writer {
        EventQueueWriter {
            is_open: self.is_open.clone(),
            sender: self.sender.clone(),
        }
    }
}

impl<T: Send> fmt::Debug for EventQueue<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventQueue")
            .field("is_open", &self.is_open.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// Consumer handle of an `EventQueue`.
///
/// Implements [`EventSinkReader`].
#[pin_project]
pub struct EventQueueReader<T: Send> {
    is_open: Arc<AtomicBool>,
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
    fn open(&mut self) {
        self.is_open.store(true, Ordering::Relaxed);
    }

    fn close(&mut self) {
        self.is_open.store(false, Ordering::Relaxed);
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
            .field("is_open", &self.is_open.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// A producer handle of an `EventQueue`.
pub struct EventQueueWriter<T: Send> {
    is_open: Arc<AtomicBool>,
    sender: mpsc::UnboundedSender<T>,
}

impl<T: Send + 'static> EventSinkWriter<T> for EventQueueWriter<T> {
    /// Pushes an event onto the queue.
    fn write(&self, event: T) {
        if !self.is_open.load(Ordering::Relaxed) {
            return;
        }
        // Ignore sending failure.
        let _ = self.sender.unbounded_send(event);
    }
}

impl<T: Send> Clone for EventQueueWriter<T> {
    fn clone(&self) -> Self {
        Self {
            is_open: self.is_open.clone(),
            sender: self.sender.clone(),
        }
    }
}

impl<T: Send> fmt::Debug for EventQueueWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventQueueWriter").finish_non_exhaustive()
    }
}
