//! Multi-threaded `async` executor.
//!
//! The executor is exclusively designed for message-passing computational
//! tasks. As such, it does not include an I/O reactor and does not consider
//! fairness as a goal in itself. While it does use fair local queues inasmuch
//! as these tend to perform better in message-passing applications, it uses an
//! unfair injection queue and a LIFO slot without attempt to mitigate the
//! effect of badly behaving code (e.g. futures that spin-lock by yielding to
//! the executor; there is for this reason no support for something like tokio's
//! `yield_now`).
//!
//! Another way in which it differs from other `async` executors is that it
//! treats deadlocking as a normal occurrence. This is because in a
//! discrete-time simulator, the simulation of a system at a given time step
//! will make as much progress as possible until it technically reaches a
//! deadlock. Only then does the simulator advance the simulated time to that of
//! the next "event" extracted from a time-sorted priority queue.
//!
//! The design of the executor is largely influenced by the tokio and Go
//! schedulers, both of which are optimized for message-passing applications. In
//! particular, it uses fast, fixed-size thread-local work-stealing queues with
//! a non-stealable LIFO slot in combination with an injector queue, which
//! injector queue is used both to schedule new tasks and to absorb temporary
//! overflow in the local queues.
//!
//! The design of the injector queue is kept very simple compared to tokio, by
//! taking advantage of the fact that the injector is not required to be either
//! LIFO or FIFO. Moving tasks between a local queue and the injector is fast
//! because tasks are moved in batch and are stored contiguously in memory.
//!
//! Another difference with tokio is that, at the moment, the complete subset of
//! active worker threads is stored in a single atomic variable. This makes it
//! possible to rapidly identify free worker threads for stealing operations,
//! with the downside that the maximum number of worker threads is currently
//! limited to `usize::BITS`. This is not expected to constitute a limitation in
//! practice since system simulation is not typically embarrassingly parallel.
//!
//! Probably the largest difference with tokio is the task system, which has
//! better throughput due to less need for synchronization. This mainly results
//! from the use of an atomic notification counter rather than an atomic
//! notification flag, thus alleviating the need to reset the notification flag
//! before polling a future.

mod injector;
mod pool_manager;

use std::cell::Cell;
use std::fmt;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// TODO: revert to `crossbeam_utils::sync::Parker` once timeout support lands in
// v1.0 (see https://github.com/crossbeam-rs/crossbeam/pull/1012).
use parking::{Parker, Unparker};
use slab::Slab;

use crate::channel;
use crate::executor::task::{self, CancelToken, Promise, Runnable};
use crate::executor::{
    ExecutorError, Signal, SimulationContext, NEXT_EXECUTOR_ID, SIMULATION_CONTEXT,
};
use crate::macros::scoped_thread_local::scoped_thread_local;
use crate::simulation::{CURRENT_MODEL_ID, MODEL_SCHEDULER};
use crate::util::rng::Rng;
use pool_manager::PoolManager;

use super::GlobalScheduler;

const BUCKET_SIZE: usize = 128;
const QUEUE_SIZE: usize = BUCKET_SIZE * 2;

type Bucket = injector::Bucket<Runnable, BUCKET_SIZE>;
type Injector = injector::Injector<Runnable, BUCKET_SIZE>;
type LocalQueue = st3::fifo::Worker<Runnable>;
type Stealer = st3::fifo::Stealer<Runnable>;

scoped_thread_local!(static LOCAL_WORKER: Worker);
scoped_thread_local!(static ACTIVE_TASKS: Mutex<Slab<CancelToken>>);

/// A multi-threaded `async` executor.
pub(crate) struct Executor {
    /// Shared executor data.
    context: Arc<ExecutorContext>,
    /// List of tasks that have not completed yet.
    active_tasks: Arc<Mutex<Slab<CancelToken>>>,
    /// Parker for the main executor thread.
    parker: Parker,
    /// Handles to the worker threads.
    worker_handles: Vec<JoinHandle<()>>,
    /// Handle to the forced termination signal.
    abort_signal: Signal,
}

impl Executor {
    /// Creates an executor that runs futures on a thread pool.
    ///
    /// The maximum number of threads is set with the `num_threads` parameter.
    ///
    /// # Panics
    ///
    /// This will panic if the specified number of threads is zero or is more
    /// than `usize::BITS`.
    pub(crate) fn new(
        num_threads: usize,
        simulation_context: SimulationContext,
        abort_signal: Signal,
        scheduler: GlobalScheduler,
    ) -> Self {
        let parker = Parker::new();
        let unparker = parker.unparker().clone();

        let (local_queues_and_parkers, stealers_and_unparkers): (Vec<_>, Vec<_>) = (0..num_threads)
            .map(|_| {
                let parker = Parker::new();
                let unparker = parker.unparker().clone();
                let local_queue = LocalQueue::new(QUEUE_SIZE);
                let stealer = local_queue.stealer();

                ((local_queue, parker), (stealer, unparker))
            })
            .unzip();

        // Each executor instance has a unique ID inherited by tasks to ensure
        // that tasks are scheduled on their parent executor.
        let executor_id = NEXT_EXECUTOR_ID.fetch_add(1, Ordering::Relaxed);
        assert!(
            executor_id <= usize::MAX / 2,
            "too many executors have been instantiated"
        );

        let context = Arc::new(ExecutorContext::new(
            executor_id,
            unparker,
            stealers_and_unparkers.into_iter(),
        ));
        let active_tasks = Arc::new(Mutex::new(Slab::new()));

        // All workers must be marked as active _before_ spawning the threads to
        // make sure that the count of active workers does not fall to zero
        // before all workers are blocked on the signal barrier.
        context.pool_manager.set_all_workers_active();

        // Spawn all worker threads.
        let worker_handles: Vec<_> = local_queues_and_parkers
            .into_iter()
            .enumerate()
            .map(|(id, (local_queue, worker_parker))| {
                let thread_builder = thread::Builder::new().name(format!("Worker #{}", id));

                thread_builder
                    .spawn({
                        let context = context.clone();
                        let active_tasks = active_tasks.clone();
                        let simulation_context = simulation_context.clone();
                        let abort_signal = abort_signal.clone();
                        let sch = scheduler.clone();

                        move || {
                            let worker = Worker::new(local_queue, context);
                            // TODO simulation context is redundant
                            SIMULATION_CONTEXT.set(&simulation_context, || {
                                MODEL_SCHEDULER.set(&sch, || {
                                    ACTIVE_TASKS.set(&active_tasks, || {
                                        LOCAL_WORKER.set(&worker, || {
                                            run_local_worker(
                                                &worker,
                                                id,
                                                worker_parker,
                                                abort_signal,
                                            )
                                        })
                                    })
                                })
                            });
                        }
                    })
                    .unwrap()
            })
            .collect();

        // Wait until all workers are blocked on the signal barrier.
        parker.park();
        assert!(context.pool_manager.pool_is_idle());

        Self {
            context,
            active_tasks,
            parker,
            worker_handles,
            abort_signal,
        }
    }

    /// Spawns a task and returns a promise that can be polled to retrieve the
    /// task's output.
    ///
    /// Note that spawned tasks are not executed until [`run`](Executor::run) is
    /// called.
    pub(crate) fn spawn<T>(&self, future: T) -> Promise<T::Output>
    where
        T: Future + Send + 'static,
        T::Output: Send + 'static,
    {
        // Book a slot to store the task cancellation token.
        let mut active_tasks = self.active_tasks.lock().unwrap();
        let task_entry = active_tasks.vacant_entry();

        // Wrap the future so that it removes its cancel token from the
        // executor's list when dropped.
        let future = CancellableFuture::new(future, task_entry.key());

        let (promise, runnable, cancel_token) =
            task::spawn(future, schedule_task, self.context.executor_id);

        task_entry.insert(cancel_token);
        self.context.injector.insert_task(runnable);

        promise
    }

    /// Spawns a task which output will never be retrieved.
    ///
    /// This is mostly useful to avoid undue reference counting for futures that
    /// return a `()` type.
    ///
    /// Note that spawned tasks are not executed until [`run`](Executor::run) is
    /// called.
    pub(crate) fn spawn_and_forget<T>(&self, future: T)
    where
        T: Future + Send + 'static,
        T::Output: Send + 'static,
    {
        // Book a slot to store the task cancellation token.
        let mut active_tasks = self.active_tasks.lock().unwrap();
        let task_entry = active_tasks.vacant_entry();

        // Wrap the future so that it removes its cancel token from the
        // executor's list when dropped.
        let future = CancellableFuture::new(future, task_entry.key());

        let (runnable, cancel_token) =
            task::spawn_and_forget(future, schedule_task, self.context.executor_id);

        task_entry.insert(cancel_token);
        self.context.injector.insert_task(runnable);
    }

    /// Execute spawned tasks, blocking until all futures have completed or an
    /// error is encountered.
    pub(crate) fn run(&mut self, timeout: Duration) -> Result<(), ExecutorError> {
        self.context.pool_manager.activate_worker();

        loop {
            if let Some((model_id, payload)) = self.context.pool_manager.take_panic() {
                return Err(ExecutorError::Panic(model_id, payload));
            }

            if self.context.pool_manager.pool_is_idle() {
                let msg_count = self.context.msg_count.load(Ordering::Relaxed);
                if msg_count != 0 {
                    let msg_count: usize = msg_count.try_into().unwrap();

                    return Err(ExecutorError::UnprocessedMessages(msg_count));
                }

                return Ok(());
            }

            if timeout.is_zero() {
                self.parker.park();
            } else if !self.parker.park_timeout(timeout) {
                // A timeout occurred: request all worker threads to return
                // as soon as possible.
                self.abort_signal.set();
                self.context.pool_manager.activate_all_workers();

                return Err(ExecutorError::Timeout);
            }
        }
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        // Force all threads to return.
        self.abort_signal.set();
        self.context.pool_manager.activate_all_workers();
        for handle in self.worker_handles.drain(0..) {
            handle.join().unwrap();
        }

        // Drop all tasks that have not completed.
        //
        // A local worker must be set because some tasks may schedule other
        // tasks when dropped, which requires that a local worker be available.
        let worker = Worker::new(LocalQueue::new(QUEUE_SIZE), self.context.clone());
        LOCAL_WORKER.set(&worker, || {
            // Cancel all pending futures.
            //
            // `ACTIVE_TASKS` is explicitly unset to prevent
            // `CancellableFuture::drop()` from trying to remove its own token
            // from the list of active tasks as this would result in a reentrant
            // lock. This is mainly to stay on the safe side: `ACTIVE_TASKS`
            // should not be set on this thread anyway, unless for some reason
            // the executor runs inside another executor.
            ACTIVE_TASKS.unset(|| {
                let mut tasks = self.active_tasks.lock().unwrap();
                for task in tasks.drain() {
                    task.cancel();
                }

                // Some of the dropped tasks may have scheduled other tasks that
                // were not yet cancelled, preventing them from being dropped
                // upon cancellation. This is OK: the scheduled tasks will be
                // dropped when the local and injector queues are dropped, and
                // they cannot re-schedule one another since all tasks were
                // cancelled.
            });
        });
    }
}

impl fmt::Debug for Executor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Executor").finish_non_exhaustive()
    }
}

/// Shared executor context.
///
/// This contains all executor resources that can be shared between threads.
struct ExecutorContext {
    /// Injector queue.
    injector: Injector,
    /// Unique executor identifier inherited by all tasks spawned on this
    /// executor instance.
    executor_id: usize,
    /// Unparker for the main executor thread.
    executor_unparker: Unparker,
    /// Manager for all worker threads.
    pool_manager: PoolManager,
    /// Difference between the number of sent and received messages.
    ///
    /// This counter is only updated by worker threads before they park and is
    /// therefore only consistent once all workers are parked.
    msg_count: AtomicIsize,
}

impl ExecutorContext {
    /// Creates a new shared executor context.
    pub(super) fn new(
        executor_id: usize,
        executor_unparker: Unparker,
        stealers_and_unparkers: impl Iterator<Item = (Stealer, Unparker)>,
    ) -> Self {
        let (stealers, worker_unparkers): (Vec<_>, Vec<_>) =
            stealers_and_unparkers.into_iter().unzip();
        let worker_unparkers = worker_unparkers.into_boxed_slice();

        Self {
            injector: Injector::new(),
            executor_id,
            executor_unparker,
            pool_manager: PoolManager::new(
                worker_unparkers.len(),
                stealers.into_boxed_slice(),
                worker_unparkers,
            ),
            msg_count: AtomicIsize::new(0),
        }
    }
}

/// A `Future` wrapper that removes its cancellation token from the list of
/// active tasks when dropped.
struct CancellableFuture<T: Future> {
    inner: T,
    cancellation_key: usize,
}

impl<T: Future> CancellableFuture<T> {
    /// Creates a new `CancellableFuture`.
    fn new(fut: T, cancellation_key: usize) -> Self {
        Self {
            inner: fut,
            cancellation_key,
        }
    }
}

impl<T: Future> Future for CancellableFuture<T> {
    type Output = T::Output;

    #[inline(always)]
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        unsafe { self.map_unchecked_mut(|s| &mut s.inner).poll(cx) }
    }
}

impl<T: Future> Drop for CancellableFuture<T> {
    fn drop(&mut self) {
        // Remove the task from the list of active tasks if the future is
        // dropped on a worker thread. Otherwise do nothing and let the
        // executor's drop handler do the cleanup.
        let _ = ACTIVE_TASKS.map(|active_tasks| {
            // Don't unwrap on `lock()` because this function can be called from
            // a destructor and should not panic. In the worse case, the cancel
            // token will be left in the list of active tasks, which does
            // prevents eager task deallocation but does not cause any issue
            // otherwise.
            if let Ok(mut active_tasks) = active_tasks.lock() {
                let _cancel_token = active_tasks.try_remove(self.cancellation_key);
            }
        });
    }
}

/// A local worker with access to global executor resources.
pub(crate) struct Worker {
    local_queue: LocalQueue,
    fast_slot: Cell<Option<Runnable>>,
    executor_context: Arc<ExecutorContext>,
}

impl Worker {
    /// Creates a new worker.
    fn new(local_queue: LocalQueue, executor_context: Arc<ExecutorContext>) -> Self {
        Self {
            local_queue,
            fast_slot: Cell::new(None),
            executor_context,
        }
    }
}

/// Schedules a `Runnable` from within a worker thread.
///
/// # Panics
///
/// This function will panic if called from a non-worker thread or if called
/// from the worker thread of another executor instance than the one the task
/// for this `Runnable` was spawned on.
fn schedule_task(task: Runnable, executor_id: usize) {
    LOCAL_WORKER
        .map(|worker| {
            let pool_manager = &worker.executor_context.pool_manager;
            let injector = &worker.executor_context.injector;
            let local_queue = &worker.local_queue;
            let fast_slot = &worker.fast_slot;

            // Check that this task was indeed spawned on this executor.
            assert_eq!(
                executor_id, worker.executor_context.executor_id,
                "Tasks must be awaken on the same executor they are spawned on"
            );

            // Store the task in the fast slot and retrieve the one that was
            // formerly stored, if any.
            let prev_task = match fast_slot.replace(Some(task)) {
                // If there already was a task in the slot, proceed so it can be
                // moved to a task queue.
                Some(t) => t,
                // Otherwise return immediately: this task cannot be stolen so
                // there is no point in activating a sibling worker.
                None => return,
            };

            // Push the previous task to the local queue if possible or on the
            // injector queue otherwise.
            if let Err(prev_task) = local_queue.push(prev_task) {
                // The local queue is full. Try to move half of it to the
                // injector queue; if this fails, just push one task to the
                // injector queue.
                if let Ok(drain) = local_queue.drain(|_| Bucket::capacity()) {
                    injector.push_bucket(Bucket::from_iter(drain));
                    local_queue.push(prev_task).unwrap();
                } else {
                    injector.insert_task(prev_task);
                }
            }

            // A task has been pushed to the local or injector queue: try to
            // activate another worker if no worker is currently searching for a
            // task.
            if pool_manager.searching_worker_count() == 0 {
                pool_manager.activate_worker_relaxed();
            }
        })
        .expect("Tasks may not be awaken outside executor threads");
}

/// Processes all incoming tasks on a worker thread until the `Terminate` signal
/// is received or until it panics.
///
/// Panics caught in this thread are relayed to the main executor thread.
fn run_local_worker(worker: &Worker, id: usize, parker: Parker, abort_signal: Signal) {
    let pool_manager = &worker.executor_context.pool_manager;
    let injector = &worker.executor_context.injector;
    let executor_unparker = &worker.executor_context.executor_unparker;
    let local_queue = &worker.local_queue;
    let fast_slot = &worker.fast_slot;

    // Update the global message counter.
    let update_msg_count = || {
        let thread_msg_count = channel::THREAD_MSG_COUNT.replace(0);
        worker
            .executor_context
            .msg_count
            .fetch_add(thread_msg_count, Ordering::Relaxed);
    };

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // Set how long to spin when searching for a task.
        const MAX_SEARCH_DURATION: Duration = Duration::from_nanos(1000);

        // Seed a thread RNG with the worker ID.
        let rng = Rng::new(id as u64);

        loop {
            // Signal barrier: park until notified to continue or terminate.

            // Try to deactivate the worker.
            if pool_manager.try_set_worker_inactive(id) {
                // No need to call `begin_worker_search()`: this was done by the
                // thread that unparked the worker.
                update_msg_count();
                parker.park();
            } else if injector.is_empty() {
                // This worker could not be deactivated because it was the last
                // active worker. In such case, the call to
                // `try_set_worker_inactive` establishes a synchronization with
                // all threads that pushed tasks to the injector queue but could
                // not activate a new worker, which is why some tasks may now be
                // visible in the injector queue.
                pool_manager.set_all_workers_inactive();
                update_msg_count();
                executor_unparker.unpark();
                parker.park();
                // No need to call `begin_worker_search()`: this was done by the
                // thread that unparked the worker.
            } else {
                pool_manager.begin_worker_search();
            }

            if abort_signal.is_set() {
                return;
            }

            let mut search_start = Instant::now();

            // Process the tasks one by one.
            loop {
                // Check the injector queue first.
                if let Some(bucket) = injector.pop_bucket() {
                    let bucket_iter = bucket.into_iter();

                    // There is a _very_ remote possibility that, even though
                    // the local queue is empty, it has temporarily too little
                    // spare capacity for the bucket. This could happen if a
                    // concurrent steal operation was preempted for all the time
                    // it took to pop and process the remaining tasks and it
                    // hasn't released the stolen capacity yet.
                    //
                    // Unfortunately, we cannot just skip checking the injector
                    // queue altogether when there isn't enough spare capacity
                    // in the local queue because this could lead to a race:
                    // suppose that (1) this thread has earlier pushed tasks
                    // onto the injector queue, and (2) the stealer has
                    // processed all stolen tasks before this thread sees the
                    // capacity restored and at the same time (3) the stealer
                    // does not yet see the tasks this thread pushed to the
                    // injector queue; in such scenario, both this thread and
                    // the stealer thread may park and leave unprocessed tasks
                    // in the injector queue.
                    //
                    // This is the only instance where spinning is used, as the
                    // probability of this happening is close to zero and the
                    // complexity of a signaling mechanism (condvar & friends)
                    // wouldn't carry its weight.
                    while local_queue.spare_capacity() < bucket_iter.len() {}

                    // Since empty buckets are never pushed onto the injector
                    // queue, we should now have at least one task to process.
                    local_queue.extend(bucket_iter);
                } else {
                    // The injector queue is empty. Try to steal from active
                    // siblings.
                    let mut stealers = pool_manager.shuffled_stealers(Some(id), &rng);
                    if stealers.all(|stealer| {
                        stealer
                            .steal_and_pop(local_queue, |n| n - n / 2)
                            .map(|(task, _)| {
                                let prev_task = fast_slot.replace(Some(task));
                                assert!(prev_task.is_none());
                            })
                            .is_err()
                    }) {
                        // Give up if unsuccessful for too long.
                        if (Instant::now() - search_start) > MAX_SEARCH_DURATION {
                            pool_manager.end_worker_search();
                            break;
                        }

                        // Re-try.
                        continue;
                    }
                }

                // Signal the end of the search so that another worker can be
                // activated when a new task is scheduled.
                pool_manager.end_worker_search();

                // Pop tasks from the fast slot or the local queue.
                while let Some(task) = fast_slot.take().or_else(|| local_queue.pop()) {
                    if abort_signal.is_set() {
                        return;
                    }
                    task.run();
                }

                // Resume the search for tasks.
                pool_manager.begin_worker_search();
                search_start = Instant::now();
            }
        }
    }));

    // Report the panic, if any.
    if let Err(payload) = result {
        let model_id = CURRENT_MODEL_ID.take();
        pool_manager.register_panic(model_id, payload);
        abort_signal.set();
        pool_manager.activate_all_workers();
        executor_unparker.unpark();
    }
}
