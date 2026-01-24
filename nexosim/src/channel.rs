//! Multiple-producer single-consumer Channel for communication between
//! simulation models.
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]

mod queue;

use std::cell::Cell;
use std::error;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use async_event::Event;
use diatomic_waker::primitives::DiatomicWaker;
use recycle_box::RecycleBox;

use queue::{PopError, PushError, Queue};
use recycle_box::coerce_box;

use crate::model::{Context, Model};

// Counts the difference between the number of sent and received messages for
// this thread.
//
// This is used by the executor to make sure that all messages have been
// received upon completion of a simulation step, i.e. that no deadlock
// occurred.
thread_local! { pub(crate) static THREAD_MSG_COUNT: Cell<isize> = const { Cell::new(0) }; }

/// Data shared between the receiver and the senders.
struct Inner<M> {
    /// Non-blocking internal queue.
    queue: Queue<dyn MessageFn<M>>,
    /// Signalling primitive used to notify the receiver.
    receiver_signal: DiatomicWaker,
    /// Signalling primitive used to notify one or several senders.
    sender_signal: Event,
}

impl<M: 'static> Inner<M> {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Queue::new(capacity),
            receiver_signal: DiatomicWaker::new(),
            sender_signal: Event::new(),
        }
    }
}

/// A receiver which can asynchronously execute `async` message that takes an
/// argument of type `&mut M` and an optional `&Context<M>` `&mut M::Env`
/// arguments.
pub(crate) struct Receiver<M> {
    /// Shared data.
    inner: Arc<Inner<M>>,
    /// A recyclable box to temporarily store the `async` closure to be
    /// executed.
    future_box: Option<RecycleBox<()>>,
}

impl<M: Model> Receiver<M> {
    /// Creates a new receiver with the specified capacity.
    ///
    /// # Panic
    ///
    /// The constructor will panic if the requested capacity is 0 or is greater
    /// than `usize::MAX/2 + 1`.
    pub(crate) fn new(capacity: usize) -> Self {
        let inner = Arc::new(Inner::new(capacity));

        Receiver {
            inner,
            future_box: Some(RecycleBox::new(())),
        }
    }

    /// Creates a new sender.
    pub(crate) fn sender(&self) -> Sender<M> {
        Sender {
            inner: self.inner.clone(),
        }
    }

    /// Creates a new observer.
    pub(crate) fn observer(&self) -> impl ChannelObserver + use<M> {
        Observer {
            inner: self.inner.clone(),
        }
    }

    /// Receives and executes a message asynchronously, if necessary waiting
    /// until one becomes available.
    pub(crate) async fn recv(
        &mut self,
        model: &mut M,
        cx: &Context<M>,
        env: &mut M::Env,
    ) -> Result<(), RecvError> {
        let msg = unsafe {
            self.inner
                .receiver_signal
                .wait_until(|| match self.inner.queue.pop() {
                    Ok(msg) => Some(Some(msg)),
                    Err(PopError::Empty) => None,
                    Err(PopError::Closed) => Some(None),
                })
                .await
        };

        match msg {
            Some(mut msg) => {
                // Decrement the count of in-flight messages.
                THREAD_MSG_COUNT.set(THREAD_MSG_COUNT.get().wrapping_sub(1));

                // Take the message to obtain a boxed future.
                let fut = msg.call_once(model, cx, env, self.future_box.take().unwrap());

                // Now that the message was taken, drop `msg` to free its slot
                // in the queue and signal to one awaiting sender that a slot is
                // available for sending.
                drop(msg);
                self.inner.sender_signal.notify_one();

                // Await the future provided by the message.
                let mut fut = RecycleBox::into_pin(fut);
                fut.as_mut().await;

                // Recycle the box.
                self.future_box = Some(RecycleBox::vacate_pinned(fut));

                Ok(())
            }
            None => Err(RecvError),
        }
    }

    /// Closes the channel.
    ///
    /// This prevents any further messages from being sent to the channel.
    /// Messages that were already sent can still be received, however, which is
    /// why a call to this method should typically be followed by a loop
    /// receiving all remaining messages.
    ///
    /// For this reason, no counterpart to [`Sender::is_closed`] is exposed by
    /// the receiver as such method could easily be misused and lead to lost
    /// messages. Instead, messages should be received until a [`RecvError`] is
    /// returned.
    #[allow(unused)]
    pub(crate) fn close(&self) {
        if !self.inner.queue.is_closed() {
            self.inner.queue.close();

            // Notify all blocked senders that the channel is closed.
            self.inner.sender_signal.notify_all();
        }
    }

    /// Returns a unique identifier for the channel.
    ///
    /// All channels are guaranteed to have different identifiers at any given
    /// time, but an identifier may be reused after all handles to a channel
    /// have been dropped.
    pub(crate) fn channel_id(&self) -> ChannelId {
        ChannelId(&*self.inner as *const Inner<M> as usize)
    }
}

impl<M> Drop for Receiver<M> {
    fn drop(&mut self) {
        self.inner.queue.close();

        // Notify all blocked senders that the channel is closed.
        self.inner.sender_signal.notify_all();
    }
}

impl<M> fmt::Debug for Receiver<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receiver").finish_non_exhaustive()
    }
}

/// A handle to a channel that can send messages.
///
/// Multiple [`Sender`] handles can be created using the [`Receiver::sender`]
/// method or via cloning.
pub(crate) struct Sender<M: 'static> {
    /// Shared data.
    inner: Arc<Inner<M>>,
}

impl<M: Model> Sender<M> {
    /// Sends a message, if necessary waiting until enough capacity becomes
    /// available in the channel.
    pub(crate) async fn send<F>(&self, msg_fn: F) -> Result<(), SendError>
    where
        F: for<'a> FnOnce(
                &'a mut M,
                &'a Context<M>,
                &'a mut M::Env,
                RecycleBox<()>,
            ) -> RecycleBox<dyn Future<Output = ()> + Send + 'a>
            + Send
            + 'static,
    {
        // Define a closure that boxes the argument in a type-erased
        // `RecycleBox`.
        let mut msg_fn = Some(|vacated_box| -> RecycleBox<dyn MessageFn<M>> {
            coerce_box!(RecycleBox::recycle(vacated_box, MessageFnOnce::new(msg_fn)))
        });

        let success = self
            .inner
            .sender_signal
            .wait_until(|| {
                match self.inner.queue.push(msg_fn.take().unwrap()) {
                    Ok(()) => Some(true),
                    Err(PushError::Full(m)) => {
                        // Recycle the message.
                        msg_fn = Some(m);

                        None
                    }
                    Err(PushError::Closed) => Some(false),
                }
            })
            .await;

        if success {
            self.inner.receiver_signal.notify();

            // Increment the count of in-flight messages.
            THREAD_MSG_COUNT.set(THREAD_MSG_COUNT.get().wrapping_add(1));

            Ok(())
        } else {
            Err(SendError)
        }
    }

    /// Closes the channel.
    ///
    /// This prevents any further messages from being sent. Messages that were
    /// already sent can still be received.
    #[allow(unused)]
    pub(crate) fn close(&self) {
        self.inner.queue.close();

        // Notify the receiver and all blocked senders that the channel is
        // closed.
        self.inner.receiver_signal.notify();
        self.inner.sender_signal.notify_all();
    }

    /// Checks if the channel is closed.
    ///
    /// This can happen either because the [`Receiver`] was dropped or because
    /// one of the [`Sender::close`] or [`Receiver::close`] method was called.
    #[allow(unused)]
    pub(crate) fn is_closed(&self) -> bool {
        self.inner.queue.is_closed()
    }

    /// Returns a unique identifier for the channel.
    ///
    /// All channels are guaranteed to have different identifiers at any given
    /// time, but an identifier may be reused after all handles to a channel
    /// have been dropped.
    pub(crate) fn channel_id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }
}

impl<M> Clone for Sender<M> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// A model-independent handle to a channel that can observe the current number
/// of messages.
pub(crate) trait ChannelObserver: Send {
    /// Returns the current number of messages in the channel.
    ///
    /// # Warning
    ///
    /// The returned result is only meaningful if it can be established than
    /// there are no concurrent send or receive operations on the channel.
    /// Otherwise, the returned value may neither reflect the current state nor
    /// the past state of the channel, and may be greater than the capacity of
    /// the channel.
    fn len(&self) -> usize;
}

/// A handle to a channel that can observe the current number of messages.
///
/// Multiple [`Observer`]s can be created using the [`Receiver::observer`]
/// method or via cloning.
#[derive(Clone)]
pub(crate) struct Observer<M: 'static> {
    /// Shared data.
    inner: Arc<Inner<M>>,
}

impl<M: Model> ChannelObserver for Observer<M> {
    fn len(&self) -> usize {
        self.inner.queue.len()
    }
}

impl<M> fmt::Debug for Sender<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Address").finish_non_exhaustive()
    }
}

/// A closure that can be called once to create a future boxed in a `RecycleBox`
/// from an `&mut M`, a `&Context<M>`, a `&mut M::Env` and an empty
/// `RecycleBox`.
///
/// This is basically a workaround to emulate an `FnOnce` with the equivalent of
/// an `FnMut` so that it is possible to call it as a `dyn` trait stored in a
/// custom pointer type like `RecycleBox` (a `Box<dyn FnOnce>` would not need
/// this because it implements the magical `DerefMove` trait and therefore can
/// be used to call an `FnOnce`).
trait MessageFn<M: Model>: Send {
    /// A method that can be executed once.
    ///
    /// # Panics
    ///
    /// This method may panic if called more than once.
    fn call_once<'a>(
        &mut self,
        model: &'a mut M,
        cx: &'a Context<M>,
        env: &'a mut M::Env,
        recycle_box: RecycleBox<()>,
    ) -> RecycleBox<dyn Future<Output = ()> + Send + 'a>;
}

/// A `MessageFn` implementation wrapping an async `FnOnce`.
struct MessageFnOnce<F, M> {
    msg_fn: Option<F>,
    _phantom: PhantomData<fn(&mut M)>,
}
impl<F, M> MessageFnOnce<F, M> {
    fn new(msg_fn: F) -> Self {
        Self {
            msg_fn: Some(msg_fn),
            _phantom: PhantomData,
        }
    }
}
impl<F, M: Model> MessageFn<M> for MessageFnOnce<F, M>
where
    F: for<'a> FnOnce(
            &'a mut M,
            &'a Context<M>,
            &'a mut M::Env,
            RecycleBox<()>,
        ) -> RecycleBox<dyn Future<Output = ()> + Send + 'a>
        + Send,
{
    fn call_once<'a>(
        &mut self,
        model: &'a mut M,
        cx: &'a Context<M>,
        env: &'a mut M::Env,
        recycle_box: RecycleBox<()>,
    ) -> RecycleBox<dyn Future<Output = ()> + Send + 'a> {
        let closure = self.msg_fn.take().unwrap();

        (closure)(model, cx, env, recycle_box)
    }
}

/// Unique identifier for a channel.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ChannelId(usize);

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An error returned when an attempt to send a message asynchronously is
/// unsuccessful.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SendError;

/// An error returned when an attempt to receive a message asynchronously is
/// unsuccessful.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecvError;

impl error::Error for RecvError {}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "receiving from a closed channel".fmt(f)
    }
}
