//! A single-value overwriting MPMC buffer.
//!
//! `SharedCell` is designed to provide the following guarantees:
//!
//! - readers always see the latest value,
//! - in case of concurrent write attempts, one writer always succeeds,
//! - in case of concurrent read and write attempts, the writer (or one of the
//!   concurrent writer, as the case may be) always succeeds.
//!
//! In particular, if a thread writes "A" and then "B", then if a thread reads
//! "A", this reader or another reader must be able to eventually read "B",
//! meaning that even if the store of "B" is concurrent with the load of "A",
//! the store must succeed.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::atomic::Ordering;

use crate::loom_exports::cell::UnsafeCell;
use crate::loom_exports::sync::atomic::AtomicUsize;

// Indicates whether a writer holds the lock.
const WRITER_LOCK: usize = 0b01;
// Indicates whether a reader holds the lock.
const READER_LOCK: usize = 0b10;

pub(crate) struct SharedCell<T> {
    /// Double buffer for the value.
    slots: [UnsafeCell<Option<T>>; 2],

    /// Index of the slot holding the latest value (0 or 1).
    ///
    /// This is only ever updated by writers.
    slot_idx: AtomicUsize,

    /// Locks for Slots 0 and 1.
    ///
    /// Bit 0 is set when one of the writer holds the lock, and bit 1 is set if
    /// a reader holds the lock. Both bits cannot be set at the same time.
    locks: [AtomicUsize; 2],
}

impl<T> SharedCell<T> {
    pub(crate) fn new() -> Self {
        Self {
            slots: [UnsafeCell::new(None), UnsafeCell::new(None)],
            slot_idx: AtomicUsize::new(0),
            locks: [AtomicUsize::new(0), AtomicUsize::new(0)],
        }
    }

    /// Attempts to write a value.
    ///
    /// While this may fail if there are concurrent writers, it is guaranteed
    /// that one of the concurrent writers will succeed. This guarantee also
    /// holds in the presence of concurrent read operations.
    pub(crate) fn try_write(&self, value: T) -> Result<(), T> {
        let slot_idx = self.slot_idx.load(Ordering::Acquire);
        let lock = &self.locks[slot_idx];

        // Try to acquire the lock for the current slot.
        //
        // Ordering: this Acquire operation synchronizes with the Release unlock
        // operation of both writers and readers.
        match lock.fetch_or(WRITER_LOCK, Ordering::Acquire) {
            0 => {
                // Safety: the lock is acquired, no concurrent reader or writer
                // can access this slot.
                unsafe {
                    self.slots[slot_idx].with_mut(|v| *v = Some(value));
                }

                // Release the lock.
                //
                // Ordering: this Release operation synchronizes with the
                // Acquire lock operations of both writers and readers.
                lock.store(0, Ordering::Release);

                return Ok(());
            }
            WRITER_LOCK => return Err(value),
            _ => {}
        }

        // The slot is locked by a reader, try the other slot.
        let slot_idx = 1 - slot_idx;
        let lock = &self.locks[slot_idx];

        // Try to acquire the lock for the other slot.
        //
        // Ordering: this Acquire operation synchronizes with the Release unlock
        // operation of both writers and readers.
        if lock.fetch_or(WRITER_LOCK, Ordering::Acquire) == 0 {
            // Safety: the lock is acquired, no concurrent reader or writer can
            // access this slot.
            unsafe {
                self.slots[slot_idx].with_mut(|v| *v = Some(value));
            }
            // Update the index of the slot with the latest value.
            self.slot_idx.store(slot_idx, Ordering::Release);

            // Release the lock.
            //
            // Ordering: this Release operation synchronizes with the
            // Acquire lock operations of both writers and readers.
            lock.store(0, Ordering::Release);

            return Ok(());
        }

        // The first slot was locked by a reader, and the second slot was either
        // locked by a writer, or by a reader. The later implies that a
        // concurrent write operation must have happened that caused the
        // reader(s) to switch slots, so in either case a concurrent write will
        // succeed or has already succeeded: we can bail out.
        Err(value)
    }

    /// Attempts to read a value.
    ///
    /// This may fail if there are concurrent writers, but is guaranteed to
    /// return the latest value otherwise.
    pub(crate) fn try_read(&self) -> Option<T> {
        let slot_idx = self.slot_idx.load(Ordering::Acquire);
        let lock = &self.locks[slot_idx];

        // Try to acquire the lock for the current slot.
        //
        // Ordering: this Acquire operation synchronizes with the Release unlock
        // operation of both writers and readers.
        if lock.fetch_or(READER_LOCK, Ordering::Acquire) == 0 {
            // Safety: the lock is acquired, no concurrent reader or writer
            // can access this slot.
            let value = unsafe { self.slots[slot_idx].with_mut(|v| (*v).take()) };

            // Release the lock.
            //
            // Ordering: this Release operation synchronizes with the
            // Acquire lock operations of both writers and readers.
            lock.store(0, Ordering::Release);

            value
        } else {
            None
        }
    }
}

impl<T> Default for SharedCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T: Send> Sync for SharedCell<T> {}

impl<T> UnwindSafe for SharedCell<T> {}
impl<T> RefUnwindSafe for SharedCell<T> {}

#[cfg(all(test, not(nexosim_loom)))]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::thread;

    #[test]
    fn shared_cell_smoke_test() {
        let cell = SharedCell::new();

        assert!(cell.try_read().is_none());
        assert!(cell.try_write(123).is_ok());
        assert_eq!(cell.try_read(), Some(123));
        assert!(cell.try_read().is_none());
    }

    #[test]
    fn shared_cell_overwrite_test() {
        let cell = SharedCell::new();

        assert!(cell.try_write(123).is_ok());
        assert!(cell.try_write(42).is_ok());
        assert_eq!(cell.try_read(), Some(42));
        assert!(cell.try_read().is_none());
    }

    #[test]
    fn shared_cell_multi_threaded_write() {
        let cell = Arc::new(SharedCell::new());

        thread::spawn({
            let cell = cell.clone();
            move || {
                assert!(cell.try_write(123).is_ok());
            }
        });

        loop {
            if let Some(v) = cell.try_read() {
                assert_eq!(v, 123);
                return;
            }
        }
    }
}

#[cfg(all(test, nexosim_loom))]
mod tests {
    use super::*;
    use crate::loom_exports::loom_builder;

    use std::sync::Arc;

    use loom::thread;

    // Makes sure that a single-threaded write always succeed, even with a
    // concurrent reader.
    #[test]
    fn loom_shared_cell_write() {
        const DEFAULT_PREEMPTION_BOUND: usize = 4;

        let builder = loom_builder(DEFAULT_PREEMPTION_BOUND);

        builder.check(move || {
            let cell = Arc::new(SharedCell::new());

            let th = thread::spawn({
                let cell = cell.clone();
                move || assert!(cell.try_write(42).is_ok())
            });

            if let Some(v) = cell.try_read() {
                assert_eq!(v, 42);
            }

            th.join().unwrap();
        });
    }

    // Makes sure that a value can be overwritten.
    #[test]
    fn loom_shared_cell_overwrite() {
        const DEFAULT_PREEMPTION_BOUND: usize = 4;

        let builder = loom_builder(DEFAULT_PREEMPTION_BOUND);

        builder.check(move || {
            let cell = Arc::new(SharedCell::new());

            let th = thread::spawn({
                let cell = cell.clone();
                move || {
                    assert!(cell.try_write(42).is_ok());
                    assert!(cell.try_write(123).is_ok());
                }
            });

            if let Some(v) = cell.try_read() {
                if v == 42 {
                    th.join().unwrap();
                    assert_eq!(cell.try_read(), Some(123));
                } else {
                    assert_eq!(v, 123);
                    th.join().unwrap();
                }
            } else {
                th.join().unwrap();

                // The following assert fails due to this Loom bug:
                //
                // https://github.com/tokio-rs/loom/issues/254
                //
                // The full test passes with this Loom PR, which is not yet
                // merged at the time of this writing:
                //
                // https://github.com/tokio-rs/loom/pull/391

                // assert_eq!(cell.try_read(), Some(123));
            }
        });
    }

    // Makes sure that when there are concurrent writers, at least one will
    // succeed.
    #[test]
    fn loom_shared_cell_concurrent_writers() {
        const DEFAULT_PREEMPTION_BOUND: usize = 4;

        let builder = loom_builder(DEFAULT_PREEMPTION_BOUND);

        builder.check(move || {
            let cell = Arc::new(SharedCell::new());

            let th1 = thread::spawn({
                let cell = cell.clone();
                move || {
                    let _ = cell.try_write(42);
                }
            });
            let th2 = thread::spawn({
                let cell = cell.clone();
                move || {
                    let _ = cell.try_write(123);
                }
            });

            th1.join().unwrap();
            th2.join().unwrap();

            // The following unwrap and assert fail due to this Loom bug:
            //
            // https://github.com/tokio-rs/loom/issues/254
            //
            // The full test passes with this Loom PR, which is not yet merged
            // at the time of this writing:
            //
            // https://github.com/tokio-rs/loom/pull/391

            // let value = cell.try_read().expect("expected a value");
            // assert!(value == 42 || value == 123);
        });
    }

    // Makes sure that when there concurrent readers do not trigger unsafety.
    #[test]
    fn loom_shared_cell_concurrent_readers() {
        const DEFAULT_PREEMPTION_BOUND: usize = 4;

        let builder = loom_builder(DEFAULT_PREEMPTION_BOUND);

        builder.check(move || {
            let cell = Arc::new(SharedCell::new());

            let th1 = thread::spawn({
                let cell = cell.clone();
                move || cell.try_read()
            });
            let th2 = thread::spawn({
                let cell = cell.clone();
                move || cell.try_read()
            });

            assert!(cell.try_write(42).is_ok());

            let _v1 = th1.join().unwrap();
            let _v2 = th2.join().unwrap();

            // The following assert fails due to this Loom bug:
            //
            // https://github.com/tokio-rs/loom/issues/254
            //
            // The full test passes with this Loom PR, which is not yet merged
            // at the time of this writing:
            //
            // https://github.com/tokio-rs/loom/pull/391

            //
            // if v1 != Some(42) && v2 != Some(42) {
            //     assert_eq!(cell.try_read(), Some(42));
            // }
        });
    }
}
