//! Observable states.
//!
//! This module contains types used to implement states automatically propagated
//! to output on change.
//!
//! The following example shows how to create an observable state with custom
//! `observe` method.
//!
//! ```rust
//! use std::time::Duration;
//!
//! use nexosim::model::{Context, InitializedModel, Model};
//! use nexosim::ports::{EventSlot, Output};
//! use nexosim::simulation::{AutoActionKey, Mailbox, SimInit};
//! use nexosim::time::MonotonicTime;
//! use nexosim_util::observable::{Observable, Observe};
//!
//! /// Processor mode ID.
//! #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
//! pub enum ModeId {
//!     #[default]
//!     Off,
//!     Idle,
//!     Processing,
//! }
//!
//! /// Processor state.
//! #[derive(Default)]
//! pub enum State {
//!     #[default]
//!     Off,
//!     Idle,
//!     Processing(AutoActionKey),
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
//! pub struct Processor {
//!     /// Mode output.
//!     pub mode: Output<ModeId>,
//!
//!     /// Internal state.
//!     state: Observable<State, ModeId>,
//! }
//!
//! impl Processor {
//!     /// Create a new processor.
//!     pub fn new() -> Self {
//!         let mode = Output::new();
//!         Self {
//!             mode: mode.clone(),
//!             state: Observable::new(mode),
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
//!     pub async fn process(&mut self, dt: u64, cx: &mut Context<Self>) {
//!         if matches!(self.state.observe(), ModeId::Idle | ModeId::Processing) {
//!             self.state
//!                 .set(State::Processing(
//!                     cx.schedule_keyed_event(Duration::from_millis(dt), Self::finish_processing, ())
//!                         .unwrap()
//!                         .into_auto(),
//!                 ))
//!                 .await;
//!         }
//!     }
//!
//!     /// Finish processing.
//!     async fn finish_processing(&mut self) {
//!         self.state.set(State::Idle).await;
//!     }
//! }
//!
//! impl Model for Processor {
//!     /// Propagate all internal states.
//!     async fn init(mut self, _: &mut Context<Self>) -> InitializedModel<Self> {
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
//!     .init(t0).unwrap()
//!     .0;
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
/// This object encapsulates state. Every state change access is propagated to
/// the output.
#[derive(Debug)]
pub struct Observable<S, T = S>
where
    S: Observe<T> + Default,
    T: Clone + Send + 'static,
{
    /// State.
    state: S,

    /// Output used for observation.
    out: Output<T>,
}

impl<S, T> Observable<S, T>
where
    S: Observe<T> + Default,
    T: Clone + Send + 'static,
{
    /// New default state.
    pub fn new(out: Output<T>) -> Self {
        Self {
            state: S::default(),
            out,
        }
    }

    /// Get state.
    pub fn get(&self) -> &S {
        &self.state
    }

    /// Set state.
    pub async fn set(&mut self, value: S) {
        self.state = value;
        self.out.send(self.state.observe()).await;
    }

    /// Modify state using mutable reference.
    pub async fn modify<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut S) -> R,
    {
        let r = f(&mut self.state);
        self.out.send(self.state.observe()).await;
        r
    }

    /// Propagate value.
    pub async fn propagate(&mut self) {
        self.out.send(self.state.observe()).await;
    }
}

impl<S, T> Deref for Observable<S, T>
where
    S: Observe<T> + Default,
    T: Clone + Send + 'static,
{
    type Target = S;

    fn deref(&self) -> &S {
        &self.state
    }
}
