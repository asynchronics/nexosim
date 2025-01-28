//! Thread handlers.
//!
//! This module contains threading related utilities.
//!

use std::fmt;
use std::marker::PhantomData;
use std::thread::{JoinHandle, Result as ThreadResult};

use nexosim::simulation::Scheduler;

type DropAction<T> = Box<dyn FnOnce(ThreadResult<T>) + Send + 'static>;

/// A thread guard.
///
/// This is a thread guard that automatically joins the thread in the
/// destructor. Additionally, a custom drop action can be specified, which will
/// be executed on the thread's execution result during the guard's
/// destruction. The thread can also be explicitly joined using the `join`
/// method.
pub struct ThreadJoiner<T> {
    /// A phantom data marker.
    _data: PhantomData<T>,

    /// The thread handle.
    handle: Option<JoinHandle<T>>,

    /// An action processing the thread result executed on drop.
    drop_action: Option<DropAction<T>>,
}

impl<T> ThreadJoiner<T> {
    /// Creates a new `ThreadJoiner`.
    pub fn new(handle: JoinHandle<T>) -> Self {
        Self {
            _data: PhantomData,
            handle: Some(handle),
            drop_action: None,
        }
    }

    /// Creates a new `ThreadJoiner` with the specified drop action.
    pub fn new_with_drop_action<F>(handle: JoinHandle<T>, drop_action: F) -> Self
    where
        F: FnOnce(ThreadResult<T>) + Send + 'static,
    {
        Self {
            _data: PhantomData,
            handle: Some(handle),
            drop_action: Some(Box::new(drop_action)),
        }
    }

    /// Joins the guarded thread.
    pub fn join(mut self) -> ThreadResult<T> {
        // Shall never be `None`.
        self.handle.take().unwrap().join()
    }
}

impl<T> Drop for ThreadJoiner<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let result = handle.join();
            if let Some(action) = self.drop_action.take() {
                action(result);
            }
        }
    }
}

impl<T> fmt::Debug for ThreadJoiner<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ThreadJoiner").finish_non_exhaustive()
    }
}

/// A simulation guard.
///
/// This is a simulation guard that automatically halts it and joins its thread
/// in the destructor. Additionally, a custom drop action can be specified,
/// which will be executed on the thread's execution result during the guard's
/// destruction. The simulation can also be explicitly stopped using the `halt`
/// method.
pub struct SimulationJoiner<T> {
    /// The simulation scheduler.
    scheduler: Scheduler,

    /// The thread guard.
    thread: Option<ThreadJoiner<T>>,
}

impl<T> SimulationJoiner<T> {
    /// Creates a new `SimulationJoiner`.
    pub fn new(scheduler: Scheduler, handle: JoinHandle<T>) -> Self {
        Self {
            scheduler,
            thread: Some(ThreadJoiner::new(handle)),
        }
    }

    /// Creates a new `SimulationJoiner` with the specified drop action.
    pub fn new_with_drop_action<F>(
        scheduler: Scheduler,
        handle: JoinHandle<T>,
        drop_action: F,
    ) -> Self
    where
        F: FnOnce(ThreadResult<T>) + Send + 'static,
    {
        Self {
            scheduler,
            thread: Some(ThreadJoiner::new_with_drop_action(handle, drop_action)),
        }
    }

    /// Stops the simulation and joins its thread.
    pub fn halt(mut self) -> ThreadResult<T> {
        self.scheduler.halt();
        // Shall never be `None`.
        self.thread.take().unwrap().join()
    }
}

impl<T> Drop for SimulationJoiner<T> {
    fn drop(&mut self) {
        self.scheduler.halt();
    }
}

impl<T> fmt::Debug for SimulationJoiner<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SimulationJoiner").finish_non_exhaustive()
    }
}
