use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};

use super::{EventSink, EventSinkReader, EventSinkWriter};

/// An event queue with an unbounded size.
///
/// Implements [`EventSink`].
///
/// Note that [`EventSinkReader`] is implemented by [`EventQueueReader`],
/// created with the [`EventQueue::into_reader`] method.
pub struct EventQueue<T: Send> {
    is_open: Arc<AtomicBool>,
    sender: Sender<T>,
    receiver: Receiver<T>,
}

impl<T: Send> EventQueue<T> {
    /// Creates an open `EventQueue`.
    pub fn new() -> Self {
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
            timeout: Duration::ZERO,
            is_blocking: false,
        }
    }

    /// Returns a consumer handle in the blocking mode.
    pub fn into_reader_blocking(self) -> EventQueueReader<T> {
        EventQueueReader {
            is_open: self.is_open,
            receiver: self.receiver,
            timeout: Duration::ZERO,
            is_blocking: true,
        }
    }

    /// Returns a consumer handle with a timeout in the blocking mode.
    pub fn into_reader_with_timeout(self, timeout: Duration) -> EventQueueReader<T> {
        EventQueueReader {
            is_open: self.is_open,
            receiver: self.receiver,
            timeout,
            is_blocking: true,
        }
    }

    /// Creates a new `EventQueue` in the specified state.
    fn new_with_state(is_open: bool) -> Self {
        let (sender, receiver) = unbounded();
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

impl<T: Send> Default for EventQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send> fmt::Debug for EventQueue<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventQueue").finish_non_exhaustive()
    }
}

/// A consumer handle of an `EventQueue`.
///
/// Implements [`EventSinkReader`]. Calls to the iterator's `next` method may be
/// blocking, depending on the configured mode.
///
/// `None` is returned when:
///
/// * all writer handles have been dropped (i.e. the `Simulation` object has
///   been dropped), or
/// * a timeout has elapsed before an event was received, or
/// * no events are currently in the queue (non-blocking mode only).
///
/// Note that even if the iterator returns `None`, it may still produce more
/// items in the future if `None` was returned (in other words, it is not a
/// [`FusedIterator`](std::iter::FusedIterator)).
pub struct EventQueueReader<T: Send> {
    is_open: Arc<AtomicBool>,
    receiver: Receiver<T>,
    timeout: Duration,
    is_blocking: bool,
}

impl<T: Send + 'static> Iterator for EventQueueReader<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_blocking {
            if self.timeout.is_zero() {
                self.receiver.recv().ok()
            } else {
                self.receiver.recv_timeout(self.timeout).ok()
            }
        } else {
            self.receiver.try_recv().ok()
        }
    }
}

impl<T: Send + 'static> EventSinkReader for EventQueueReader<T> {
    fn open(&mut self) {
        self.is_open.store(true, Ordering::Relaxed);
    }

    fn close(&mut self) {
        self.is_open.store(false, Ordering::Relaxed);
    }

    fn set_blocking(&mut self, blocking: bool) {
        self.is_blocking = blocking;
    }

    fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }
}

impl<T: Send> Clone for EventQueueReader<T> {
    fn clone(&self) -> Self {
        Self {
            is_open: self.is_open.clone(),
            receiver: self.receiver.clone(),
            timeout: self.timeout,
            is_blocking: self.is_blocking,
        }
    }
}

impl<T: Send> fmt::Debug for EventQueueReader<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventQueueReader").finish_non_exhaustive()
    }
}

/// A producer handle of an `EventQueue`.
pub struct EventQueueWriter<T: Send> {
    is_open: Arc<AtomicBool>,
    sender: Sender<T>,
}

impl<T: Send + 'static> EventSinkWriter<T> for EventQueueWriter<T> {
    /// Pushes an event onto the queue.
    fn write(&self, event: T) {
        if !self.is_open.load(Ordering::Relaxed) {
            return;
        }
        // Ignore sending failure.
        let _ = self.sender.send(event);
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
