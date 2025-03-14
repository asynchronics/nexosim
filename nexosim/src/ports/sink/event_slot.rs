use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, TryLockError, TryLockResult};
use std::time::{Duration, Instant};

#[allow(deprecated)]
use super::EventSinkStream;
use super::{EventSink, EventSinkReader, EventSinkWriter};

/// The shared data of an `EventSlot`.
struct Inner<T> {
    is_open: AtomicBool,
    slot: Mutex<Option<T>>,
    waker: Condvar,
}

/// An iterator implementing [`EventSink`] and [`EventSinkReader`] that only
/// keeps the last event.
///
/// Once the value is read, the iterator will return `None` until a new value is
/// received. If the slot contains a value when a new value is received, the
/// previous value is overwritten.
///
/// The read operation can be blocking or non-blocking depending on mode.
///
/// `None` is returned when
/// * all writer handles have been dropped (i.e. the `Simulation` object has
///   been dropped);
/// * on timeout if one has been set;
/// * when there are no events at the moment (in the non-blocking mode).
pub struct EventSlot<T> {
    inner: Arc<Inner<T>>,
    is_blocking: bool,
    timeout: Duration,
}

impl<T> EventSlot<T> {
    /// Creates an open `EventSlot`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                is_open: AtomicBool::new(true),
                slot: Mutex::new(None),
                waker: Condvar::new(),
            }),
            is_blocking: false,
            timeout: Duration::ZERO,
        }
    }

    /// Creates a closed `EventSlot`.
    pub fn new_closed() -> Self {
        Self {
            inner: Arc::new(Inner {
                is_open: AtomicBool::new(false),
                slot: Mutex::new(None),
                waker: Condvar::new(),
            }),
            is_blocking: false,
            timeout: Duration::ZERO,
        }
    }

    /// Creates an open blocking `EventSlot`.
    pub fn new_blocking() -> Self {
        Self {
            inner: Arc::new(Inner {
                is_open: AtomicBool::new(true),
                slot: Mutex::new(None),
                waker: Condvar::new(),
            }),
            is_blocking: true,
            timeout: Duration::ZERO,
        }
    }
}

impl<T: Send + 'static> EventSink<T> for EventSlot<T> {
    type Writer = EventSlotWriter<T>;

    /// Returns a writer handle.
    fn writer(&self) -> EventSlotWriter<T> {
        EventSlotWriter {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Iterator for EventSlot<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_blocking {
            let mut slot = self.inner.slot.lock().unwrap();
            let start = Instant::now();
            while slot.is_none() {
                if self.timeout.is_zero() {
                    slot = self.inner.waker.wait(slot).unwrap();
                } else {
                    let (sl, timeout_result) =
                        self.inner.waker.wait_timeout(slot, self.timeout).unwrap();
                    slot = sl;
                    if timeout_result.timed_out() || start.elapsed() >= self.timeout {
                        break;
                    }
                }
            }
            slot.take()
        } else {
            match self.inner.slot.try_lock() {
                TryLockResult::Ok(mut v) => v.take(),
                TryLockResult::Err(TryLockError::WouldBlock) => None,
                TryLockResult::Err(TryLockError::Poisoned(_)) => panic!(),
            }
        }
    }
}

#[allow(deprecated)]
impl<T: Send + 'static> EventSinkStream for EventSlot<T> {
    fn open(&mut self) {
        self.inner.is_open.store(true, Ordering::Relaxed);
    }
    fn close(&mut self) {
        self.inner.is_open.store(false, Ordering::Relaxed);
    }
}

impl<T: Send + 'static> EventSinkReader for EventSlot<T> {
    fn open(&mut self) {
        self.inner.is_open.store(true, Ordering::Relaxed);
    }

    fn close(&mut self) {
        self.inner.is_open.store(false, Ordering::Relaxed);
    }

    fn set_blocking(&mut self, blocking: bool) {
        self.is_blocking = blocking;
    }

    fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }
}

impl<T> Default for EventSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for EventSlot<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            is_blocking: self.is_blocking,
            timeout: self.timeout,
        }
    }
}

impl<T> fmt::Debug for EventSlot<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventSlot").finish_non_exhaustive()
    }
}

/// A writer handle of an `EventSlot`.
pub struct EventSlotWriter<T> {
    inner: Arc<Inner<T>>,
}

impl<T: Send + 'static> EventSinkWriter<T> for EventSlotWriter<T> {
    /// Write an event into the slot.
    fn write(&self, event: T) {
        // Ignore if the sink is closed.
        if !self.inner.is_open.load(Ordering::Relaxed) {
            return;
        }

        // TODO: recheck this taking into account current design and use-cases.
        //
        // Why do we just use `try_lock` and abandon if the lock is taken? The
        // reason is that (i) the reader is never supposed to access the slot
        // when the simulation runs and (ii) as a rule the simulator does not
        // warrant fairness when concurrently writing to an input. Therefore, if
        // the mutex is already locked when this writer attempts to lock it, it
        // means another writer is concurrently writing an event, and that event
        // is just as legitimate as ours so there is not need to overwrite it.
        match self.inner.slot.try_lock() {
            TryLockResult::Ok(mut v) => {
                *v = Some(event);
                self.inner.waker.notify_one();
            }
            TryLockResult::Err(TryLockError::WouldBlock) => {}
            TryLockResult::Err(TryLockError::Poisoned(_)) => panic!(),
        }
    }
}

impl<T> Clone for EventSlotWriter<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> fmt::Debug for EventSlotWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventStreamWriter").finish_non_exhaustive()
    }
}
