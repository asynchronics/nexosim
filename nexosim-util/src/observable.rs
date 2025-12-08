//! Observable states.
//!
//! This module contains types that enable the automatic propagation of state
//! changes to an associated output.
//!
//! # Examples
//!
//! ## Simple observable value
//!
//! ```rust
//! use nexosim::model::{Context, InitializedModel};
//! use nexosim::ports::{EventSlot, Output};
//! use nexosim::simulation::{Mailbox, SimInit};
//! use nexosim::time::MonotonicTime;
//! use nexosim_util::observable::Observable;
//! use nexosim::Model;
//!
//! use serde::{Serialize, Deserialize};
//!
//! /// Initial count.
//! const INITIAL: u64 = 5;
//!
//! /// The `Counter` Model.
//! #[derive(Serialize, Deserialize)]
//! pub struct Counter {
//!     /// Pulses count.
//!     pub count: Output<u64>,
//!     /// Counter.
//!     acc: Observable<u64>,
//! }
//! #[Model]
//! impl Counter {
//!     /// Creates a new `Counter` model with initial count.
//!     fn new(initial_count: u64) -> Self {
//!         let count = Output::default();
//!         Self {
//!             count: count.clone(),
//!             acc: Observable::new(count, initial_count),
//!         }
//!     }
//!
//!     /// Pulse -- input port.
//!     pub async fn pulse(&mut self) {
//!         self.acc.modify(|x| *x += 1).await;
//!     }
//!
//!     #[nexosim(init)]
//!     /// Propagate the internal state.
//!     async fn init(mut self, _: &Context<Self>, _: &mut ()) -> InitializedModel<Self> {
//!         self.acc.propagate().await;
//!         self.into()
//!     }
//! }
//!
//! // ---------------
//! // Bench assembly.
//! // ---------------
//!
//! // Models.
//!
//! // The counter model.
//! let mut counter = Counter::new(INITIAL);
//!
//! // Mailboxes.
//! let counter_mbox = Mailbox::new();
//!
//! // Model handles for simulation.
//! let counter_addr = counter_mbox.address();
//! let mut count = EventSlot::new();
//! counter.count.connect_sink(&count);
//!
//! // Start time (arbitrary since models do not depend on absolute time).
//! let t0 = MonotonicTime::EPOCH;
//!
//! // Assembly and initialization.
//! let mut simu = SimInit::new()
//!     .add_model(counter, counter_mbox, "counter")
//!     .init(t0).unwrap();
//!
//! // ----------
//! // Simulation.
//! // ----------
//!
//! // The initial state.
//! assert_eq!(count.next(), Some(INITIAL));
//!
//! // Count one pulse.
//! simu.process_event(Counter::pulse, (), &counter_addr).unwrap();
//! assert_eq!(count.next(), Some(INITIAL + 1));
//! ```
//!
//! ## Custom `Observe` trait implementation
//!
//! The following example shows how to create an observable state with a custom
//! `observe` method.
//!
//! ```rust
//! use std::time::Duration;
//!
//! use serde::{Serialize, Deserialize};
//!
//! use nexosim::model::{Context, InitializedModel};
//! use nexosim::ports::{EventSlot, Output};
//! use nexosim::simulation::{AutoEventKey, Mailbox, SimInit};
//! use nexosim::time::MonotonicTime;
//! use nexosim_util::observable::{Observable, Observe};
//! use nexosim::{schedulable, Model};
//!
//! /// Processor mode ID.
//! #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
//! pub enum ModeId {
//!     #[default]
//!     Off,
//!     Idle,
//!     Processing,
//! }
//!
//! /// Processor state.
//! #[derive(Default, Serialize, Deserialize)]
//! pub enum State {
//!     #[default]
//!     Off,
//!     Idle,
//!     Processing(AutoEventKey),
//! }
//!
//! impl Observe<ModeId> for State {
//!     fn observe(&self) -> ModeId {
//!         match *self {
//!             State::Off => ModeId::Off,
//!             State::Idle => ModeId::Idle,
//!             State::Processing(_) => ModeId::Processing,
//!         }
//!     }
//! }
//!
//! /// Processor model.
//! #[derive(Serialize, Deserialize)]
//! pub struct Processor {
//!     /// Mode output.
//!     pub mode: Output<ModeId>,
//!
//!     /// Internal state.
//!     state: Observable<State, ModeId>,
//! }
//!
//! #[Model]
//! impl Processor {
//!     /// Create a new processor.
//!     pub fn new() -> Self {
//!         let mode = Output::new();
//!         Self {
//!             mode: mode.clone(),
//!             state: Observable::with_default(mode),
//!         }
//!     }
//!
//!     /// Switch processor ON/OFF.
//!     pub async fn switch_power(&mut self, on: bool) {
//!         if on {
//!             self.state.set(State::Idle).await;
//!         } else {
//!             self.state.set(State::Off).await;
//!         }
//!     }
//!
//!     /// Process data for dt milliseconds.
//!     pub async fn process(&mut self, dt: u64, cx: &Context<Self>) {
//!         if matches!(self.state.observe(), ModeId::Idle | ModeId::Processing) {
//!             self.state
//!                 .set(State::Processing(
//!                     cx.schedule_keyed_event(Duration::from_millis(dt), schedulable!(Self::finish_processing), ())
//!                         .unwrap()
//!                         .into_auto(),
//!                 ))
//!                 .await;
//!         }
//!     }
//!
//!     /// Finish processing.
//!     #[nexosim(schedulable)]
//!     async fn finish_processing(&mut self) {
//!         self.state.set(State::Idle).await;
//!     }
//!
//!     #[nexosim(init)]
//!     /// Propagate all internal states.
//!     async fn init(mut self, _: &Context<Self>, _: &mut ()) -> InitializedModel<Self> {
//!         self.state.propagate().await;
//!         self.into()
//!     }
//! }
//!
//! // ---------------
//! // Bench assembly.
//! // ---------------
//!
//! // Models.
//! let mut proc = Processor::new();
//!
//! // Mailboxes.
//! let proc_mbox = Mailbox::new();
//!
//! // Model handles for simulation.
//! let mut mode = EventSlot::new_blocking();
//!
//! proc.mode.connect_sink(&mode);
//! let proc_addr = proc_mbox.address();
//!
//! // Start time (arbitrary since models do not depend on absolute time).
//! let t0 = MonotonicTime::EPOCH;
//!
//! // Assembly and initialization.
//! let mut simu = SimInit::new()
//!     .add_model(proc, proc_mbox, "proc")
//!     .init(t0).unwrap();
//!
//! // ----------
//! // Simulation.
//! // ----------
//! assert_eq!(mode.next(), Some(ModeId::Off));
//!
//! // Switch processor on.
//! simu.process_event(Processor::switch_power, true, &proc_addr).unwrap();
//! assert_eq!(mode.next(), Some(ModeId::Idle));
//!
//! // Trigger processing.
//! simu.process_event(Processor::process, 100, &proc_addr).unwrap();
//! assert_eq!(mode.next(), Some(ModeId::Processing));
//!
//! // All data processed.
//! simu.step_until(Duration::from_millis(101)).unwrap();
//! assert_eq!(mode.next(), Some(ModeId::Idle));
//! ```

use std::ops::Deref;

use serde::{Deserialize, Serialize};

use nexosim::ports::Output;

/// Observability trait.
pub trait Observe<T> {
    /// Observe the value.
    fn observe(&self) -> T;
}

impl<T> Observe<T> for T
where
    T: Clone,
{
    fn observe(&self) -> Self {
        self.clone()
    }
}

/// Observable state.
///
/// This object encapsulates a state. Every state change is propagated to the
/// associated output.
#[derive(Debug, Serialize, Deserialize)]
pub struct Observable<S, T = S>
where
    S: Observe<T>,
    T: Clone + Send + 'static,
{
    /// State.
    state: S,

    /// Output to which the state is to be propagated.
    out: Output<T>,
}

impl<S, T> Observable<S, T>
where
    S: Observe<T>,
    T: Clone + Send + 'static,
{
    /// Constructs an `Observable`.
    pub fn new(out: Output<T>, state: S) -> Self {
        Self { state, out }
    }

    /// Returns the contained state.
    pub fn get(&self) -> &S {
        &self.state
    }

    /// Sets the contained state.
    pub async fn set(&mut self, state: S) {
        self.state = state;
        self.out.send(self.state.observe()).await;
    }

    /// Updates the contained state using a function and propagates the state
    /// set by this function. Forwards the value returned by the modification
    /// function.
    pub async fn modify<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut S) -> R,
    {
        let r = f(&mut self.state);
        self.out.send(self.state.observe()).await;
        r
    }

    /// Propagates the value.
    ///
    /// This is most typically used to propagate the value at model
    /// initialization time.
    pub async fn propagate(&mut self) {
        self.out.send(self.state.observe()).await;
    }
}

impl<S, T> Observable<S, T>
where
    S: Observe<T> + Default,
    T: Clone + Send + 'static,
{
    /// Constructs an `Observable` containing the default state.
    pub fn with_default(out: Output<T>) -> Self {
        Self {
            state: S::default(),
            out,
        }
    }
}

impl<S, T> Deref for Observable<S, T>
where
    S: Observe<T>,
    T: Clone + Send + 'static,
{
    type Target = S;

    fn deref(&self) -> &S {
        &self.state
    }
}
