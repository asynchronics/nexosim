use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use super::{EventSink, EventSinkStream, EventSinkWriter};

/// A blocking event queue with an unbounded size.
///
/// Implements [`EventSink`].
///
/// Note that [`EventSinkStream`] is implemented by
/// [`BlockingEventQueueReader`], created with the
/// [`BlockingEventQueue::into_reader`] method.
#[deprecated = "use `EventQueue` instead"]
pub struct BlockingEventQueue<T> {
    is_open: Arc<AtomicBool>,
    sender: Sender<T>,
    receiver: Receiver<T>,
}

impl<T> BlockingEventQueue<T> {
    /// Creates an open `BlockingEventQueue`.
    pub fn new() -> Self {
        Self::new_with_state(true)
    }

    /// Creates a closed `BlockingEventQueue`.
    pub fn new_closed() -> Self {
        Self::new_with_state(false)
    }

    /// Returns a consumer handle.
    pub fn into_reader(self) -> BlockingEventQueueReader<T> {
        self.into_reader_with_timeout(Duration::ZERO)
    }

    /// Returns a consumer handle with a timeout set.
    pub fn into_reader_with_timeout(self, timeout: Duration) -> BlockingEventQueueReader<T> {
        BlockingEventQueueReader {
            is_open: self.is_open,
            receiver: self.receiver,
            timeout,
        }
    }

    /// Creates a new `BlockingEventQueue` in the specified state.
    fn new_with_state(is_open: bool) -> Self {
        let (sender, receiver) = channel();
        Self {
            is_open: Arc::new(AtomicBool::new(is_open)),
            sender,
            receiver,
        }
    }
}

impl<T: Send + 'static> EventSink<T> for BlockingEventQueue<T> {
    type Writer = BlockingEventQueueWriter<T>;

    fn writer(&self) -> Self::Writer {
        BlockingEventQueueWriter {
            is_open: self.is_open.clone(),
            sender: self.sender.clone(),
        }
    }
}

impl<T> Default for BlockingEventQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for BlockingEventQueue<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BlockingEventQueue").finish_non_exhaustive()
    }
}

/// A consumer handle of a `BlockingEventQueue`.
///
/// Implements [`EventSinkStream`]. Calls to the iterator's `next` method are
/// blocking. `None` is returned when all writer handles have been dropped
/// (i.e. the `Simulation` object has been dropped) or on timeout if one has
/// been set.  Note that even if the iterator returns `None`, it may still
/// produce more items in the future if `None` was returned due to timeout (in
/// other words, it is not a [`FusedIterator`](std::iter::FusedIterator)).
#[deprecated = "use `EventQueueReader` instead"]
pub struct BlockingEventQueueReader<T> {
    is_open: Arc<AtomicBool>,
    receiver: Receiver<T>,
    timeout: Duration,
}

impl<T> BlockingEventQueueReader<T> {
    /// Sets a timeout, or cancels it if the duration is zero.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }
}

impl<T> Iterator for BlockingEventQueueReader<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.timeout.is_zero() {
            self.receiver.recv().ok()
        } else {
            self.receiver.recv_timeout(self.timeout).ok()
        }
    }
}

impl<T: Send + 'static> EventSinkStream for BlockingEventQueueReader<T> {
    fn open(&mut self) {
        self.is_open.store(true, Ordering::Relaxed);
    }

    fn close(&mut self) {
        self.is_open.store(false, Ordering::Relaxed);
    }
}

impl<T> fmt::Debug for BlockingEventQueueReader<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BlockingEventQueueReader")
            .finish_non_exhaustive()
    }
}

/// A producer handle of a `BlockingEventQueue`.
#[deprecated = "use `EventQueueWriter` instead"]
pub struct BlockingEventQueueWriter<T> {
    is_open: Arc<AtomicBool>,
    sender: Sender<T>,
}

impl<T: Send + 'static> EventSinkWriter<T> for BlockingEventQueueWriter<T> {
    /// Pushes an event onto the queue.
    fn write(&self, event: T) {
        if !self.is_open.load(Ordering::Relaxed) {
            return;
        }
        // Ignore sending failure.
        let _ = self.sender.send(event);
    }
}

impl<T> Clone for BlockingEventQueueWriter<T> {
    fn clone(&self) -> Self {
        Self {
            is_open: self.is_open.clone(),
            sender: self.sender.clone(),
        }
    }
}

impl<T> fmt::Debug for BlockingEventQueueWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("BlockingEventQueueWriter")
            .finish_non_exhaustive()
    }
}
