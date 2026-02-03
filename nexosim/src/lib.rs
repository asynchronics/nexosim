//! A high-performance, discrete-event computation framework for system
//! simulation.
//!
//! NeXosim is a developer-friendly, yet highly optimized software simulator
//! able to scale to very large simulation with complex time-driven state
//! machines.
//!
//! It promotes a component-oriented architecture that is familiar to system
//! engineers and closely resembles [flow-based programming][FBP]: a model is
//! essentially an isolated entity with a fixed set of typed inputs and outputs,
//! communicating with other models through message passing via connections
//! defined during bench assembly. Unlike in conventional flow-based
//! programming, request-reply patterns are also possible.
//!
//! NeXosim leverages asynchronous programming to perform auto-parallelization
//! in a manner that is fully transparent to model authors and users, achieving
//! high computational throughput on large simulation benches by means of a
//! custom multi-threaded executor.
//!
//!
//! [FBP]: https://en.wikipedia.org/wiki/Flow-based_programming
//!
//! # A practical overview
//!
//! Simulating a system typically involves three distinct activities:
//!
//! 1. the design of simulation models for each node of the system,
//! 2. the assembly of a simulation bench from a set of models, performed by
//!    inter-connecting model ports,
//! 3. the execution of the simulation, managed through periodical increments of
//!    the simulation time and by exchange of messages with simulation models.
//!
//! The following sections go through each of these activities in more details.
//!
//! ## Authoring models
//!
//! Models can contain four kinds of ports:
//!
//! * _output ports_, which are instances of the [`Output`](ports::Output) type
//!   and can be used to broadcast a message,
//! * _requestor ports_, which are instances of the
//!   [`UniRequestor`](ports::UniRequestor) or [`Requestor`](ports::Requestor)
//!   types and can be used to send/broadcast a message and receive a single
//!   reply ([`UniRequestor`](ports::UniRequestor)) or an iterator over the
//!   replies of all connected replier ports ([`Requestor`](ports::Requestor)),
//! * _input ports_, which are synchronous or asynchronous methods that
//!   implement the [`InputFn`](ports::InputFn) trait and take an `&mut self`
//!   argument, a message argument and, optionally, [`&Context`](model::Context)
//!   and [`&Model::Env`](model::Model::Env) arguments,
//! * _replier ports_, which are similar to input ports but implement the
//!   [`ReplierFn`](ports::ReplierFn) trait and return a reply.
//!
//! Messages that are broadcast by an output port to an input port are referred
//! to as *events*, while messages exchanged between requestor and replier ports
//! are referred to as *requests* and *replies*.
//!
//! Models must implement the [`Model`](trait@model::Model) trait, which is most
//! conveniently done by annotating the `impl` block of the model with the
//! [`#[Model]`](macro@model::Model) macro. This trait allows models to specify
//! a custom [`Model::init`](model::Model::init) method that is guaranteed to
//! run exactly once when the simulation is initialized, _i.e._ after all models
//! have been connected but before the simulation starts.
//!
//! More complex models can be built with the [`ProtoModel`](model::ProtoModel)
//! trait. The [`ProtoModel::build`](model::ProtoModel::build) method makes it
//! possible to:
//!
//! * build the final [`Model`](trait@model::Model) from a builder (the *model
//!   prototype*),
//! * perform possibly blocking actions when the model is added to the
//!   simulation rather than when the simulation starts, such as establishing a
//!   network connection or configuring hardware devices,
//! * connect submodels and add them to the simulation.
//!
//! In typical scenarios the [`Model`](trait@model::Model) trait can be
//! implemented by a [`Model`](macro@model::Model) proc-macro, applied to the
//! main `impl` block of the model struct. Definition for the `init` method
//! can be provided by using a custom `#[nexosim(init)]` attribute.
//! Moreover, input methods can be decorated with
//! `#[nexosim(schedulable)]` attribute to allow convenient self-scheduling
//! within the model.
//!
//! ### A simple model
//!
//! Let us consider for illustration a simple model that forwards its input
//! after multiplying it by 2. This model has only one input and one output
//! port:
//!
//! ```text
//!                ┌────────────┐
//!                │            │
//! Input ●───────►│ Multiplier ├───────► Output
//!          f64   │            │  f64
//!                └────────────┘
//! ```
//!
//! `Multiplier` could be implemented as follows:
//!
//! ```
//! use serde::{Deserialize, Serialize};
//! use nexosim::model::Model;
//! use nexosim::ports::Output;
//!
//! #[derive(Default, Serialize, Deserialize)]
//! pub struct Multiplier {
//!     pub output: Output<f64>,
//! }
//! #[Model]
//! impl Multiplier {
//!     pub async fn input(&mut self, value: f64) {
//!         self.output.send(2.0 * value).await;
//!     }
//! }
//! ```
//!
//! ### A model using the local context
//!
//! Models frequently need to schedule actions at a future time or simply get
//! access to the current simulation time. To do so, input and replier methods
//! can take an optional argument that gives them access to a local context.
//!
//! To show how the local context can be used in practice, let us implement a
//! `Delay` model which simply forwards its input after a 1s delay. Note as well
//! the use of the [`schedulable!`](model::schedulable) macro which, together
//! with the `[nexosim(Schedulable)]` attribute, make it possible for a model to
//! self-schedule its inputs.
//!
//! ```
//! use std::time::Duration;
//! use serde::{Deserialize, Serialize};
//! use nexosim::model::{Context, Model, schedulable};
//! use nexosim::ports::Output;
//!
//! #[derive(Default, Serialize, Deserialize)]
//! pub struct Delay {
//!    pub output: Output<f64>,
//! }
//! #[Model]
//! impl Delay {
//!     pub fn input(&mut self, value: f64, cx: &Context<Self>) {
//!         cx.schedule_event(Duration::from_secs(1), schedulable!(Self::send), value).unwrap();
//!     }
//!
//!     #[nexosim(schedulable)]
//!     async fn send(&mut self, value: f64) {
//!         self.output.send(value).await;
//!     }
//! }
//! ```
//!
//! ## Assembling simulation benches
//!
//! A simulation bench is a system of inter-connected models that have been
//! migrated to a simulation.
//!
//! The assembly process usually starts with the instantiation of models and the
//! creation of a [`Mailbox`](simulation::Mailbox) for each model. A mailbox is
//! essentially a fixed-capacity buffer for events and requests. While each
//! model has only one mailbox, it is possible to create an arbitrary number of
//! [`Address`](simulation::Mailbox)es pointing to that mailbox. For
//! convenience, methods such as [`Output::connect`](ports::Output::connect)
//! accept as the target either a [`&Mailbox`](simulation::Mailbox) reference
//! from which an address is created, or a pre-instantiated
//! [`Address`](simulation::Mailbox).
//!
//! Addresses are used among others to connect models: each output or requestor
//! port has a `connect` method that takes as argument a function pointer to
//! the corresponding input or replier port method and the address of the
//! targeted model.
//!
//! Once all models are connected, they are added to a
//! [`SimInit`](simulation::SimInit) instance, which is a builder type for the
//! final [`Simulation`](simulation::Simulation).
//!
//! The easiest way to understand the assembly step is with a short example. Say
//! that we want to assemble the following system from the models implemented
//! above:
//!
//! ```text
//!                                ┌────────────┐
//!                                │            │
//!                            ┌──►│   Delay    ├──┐
//!           ┌────────────┐   │   │            │  │   ┌────────────┐
//!           │            │   │   └────────────┘  │   │            │
//! Input ●──►│ Multiplier ├───┤                   ├──►│   Delay    ├──► Output
//!           │            │   │   ┌────────────┐  │   │            │
//!           └────────────┘   │   │            │  │   └────────────┘
//!                            └──►│ Multiplier ├──┘
//!                                │            │
//!                                └────────────┘
//! ```
//!
//! Here is how this could be done:
//!
//! ```
//! # mod models {
//! #     use std::time::Duration;
//! #     use serde::{Deserialize, Serialize};
//! #     use nexosim::model::{Context, Model, schedulable};
//! #     use nexosim::ports::Output;
//! #     #[derive(Default, Serialize, Deserialize)]
//! #     pub struct Multiplier {
//! #         pub output: Output<f64>,
//! #     }
//! #     #[Model]
//! #     impl Multiplier {
//! #         pub async fn input(&mut self, value: f64) {
//! #             self.output.send(2.0 * value).await;
//! #         }
//! #     }
//! #     #[derive(Default, Serialize, Deserialize)]
//! #     pub struct Delay {
//! #        pub output: Output<f64>,
//! #     }
//! #     #[Model]
//! #     impl Delay {
//! #         pub fn input(&mut self, value: f64, cx: &Context<Self>) {
//! #             cx.schedule_event(Duration::from_secs(1), schedulable!(Self::send), value).unwrap();
//! #         }
//! #         #[nexosim(schedulable)]
//! #         async fn send(&mut self, value: f64) { // this method can be private
//! #             self.output.send(value).await;
//! #         }
//! #     }
//! # }
//! use std::time::Duration;
//! use nexosim::ports::{EventSource, SinkState, event_slot};
//! use nexosim::simulation::{Mailbox, SimInit};
//! use nexosim::time::MonotonicTime;
//!
//! use models::{Delay, Multiplier};
//!
//! // Instantiate models.
//! let mut multiplier1 = Multiplier::default();
//! let mut multiplier2 = Multiplier::default();
//! let mut delay1 = Delay::default();
//! let mut delay2 = Delay::default();
//!
//! // Instantiate mailboxes.
//! let multiplier1_mbox = Mailbox::new();
//! let multiplier2_mbox = Mailbox::new();
//! let delay1_mbox = Mailbox::new();
//! let delay2_mbox = Mailbox::new();
//!
//! // Connect the models.
//! multiplier1.output.connect(Delay::input, &delay1_mbox);
//! multiplier1.output.connect(Multiplier::input, &multiplier2_mbox);
//! multiplier2.output.connect(Delay::input, &delay2_mbox);
//! delay1.output.connect(Delay::input, &delay2_mbox);
//!
//! // Keep handles to the system input and output for the simulation.
//! let mut bench = SimInit::new();
//!
//! let input = EventSource::new()
//!     .connect(Multiplier::input, &multiplier1_mbox)
//!     .register(&mut bench);
//!
//! let (sink, mut output) = event_slot(SinkState::Enabled);
//! delay2.output.connect_sink(sink);
//!
//! // Pick an arbitrary simulation start time and build the simulation.
//! let t0 = MonotonicTime::EPOCH;
//! let mut simu = bench
//!     .add_model(multiplier1, multiplier1_mbox, "multiplier1")
//!     .add_model(multiplier2, multiplier2_mbox, "multiplier2")
//!     .add_model(delay1, delay1_mbox, "delay1")
//!     .add_model(delay2, delay2_mbox, "delay2")
//!     .init(t0)?;
//!
//! # Ok::<(), nexosim::simulation::SimulationError>(())
//! ```
//!
//! ## Running simulations
//!
//! The simulation can be controlled in several ways:
//!
//! 1. by advancing time, either until the next scheduled event with
//!    [`Simulation::step`](simulation::Simulation::step), until a specific
//!    deadline with
//!    [`Simulation::step_until`](simulation::Simulation::step_until), or until
//!    there are no more scheduled events with
//!    [`Simulation::run`](simulation::Simulation::run).
//! 2. by sending events or queries without advancing simulation time, using
//!    [`Simulation::process_event`](simulation::Simulation::process_event) or
//!    [`Simulation::send_query`](simulation::Simulation::process_query),
//! 3. by scheduling events with a [`Scheduler`](simulation::Scheduler).
//!
//! When initialized with the default clock, the simulation will run as fast as
//! possible, without regard for the actual wall clock time. Alternatively, the
//! simulation time can be synchronized to the wall clock time using
//! [`SimInit::with_clock`](simulation::SimInit::with_clock) and providing a
//! custom [`Clock`](time::Clock) type or a readily-available real-time clock
//! such as [`AutoSystemClock`](time::AutoSystemClock).
//!
//! Simulation outputs can be monitored using
//! [`event_slot`](ports::event_slot)s, [`event_queue`](ports::event_queue)s, or
//! any implementer of the [`EventSinkWriter`](ports::EventSinkWriter) trait
//! connected to one or several model output ports.
//!
//! This is an example of simulation that could be performed using the above
//! bench assembly:
//!
//! ```
//! # mod models {
//! #     use std::time::Duration;
//! #     use serde::{Deserialize, Serialize};
//! #     use nexosim::model::{schedulable, Context, Model};
//! #     use nexosim::ports::Output;
//! #     #[derive(Default, Serialize, Deserialize)]
//! #     pub struct Multiplier {
//! #         pub output: Output<f64>,
//! #     }
//! #     #[Model]
//! #     impl Multiplier {
//! #         pub async fn input(&mut self, value: f64) {
//! #             self.output.send(2.0 * value).await;
//! #         }
//! #     }
//! #     #[derive(Default, Serialize, Deserialize)]
//! #     pub struct Delay {
//! #        pub output: Output<f64>,
//! #     }
//! #     #[Model]
//! #     impl Delay {
//! #         pub fn input(&mut self, value: f64, cx: &Context<Self>) {
//! #             cx.schedule_event(Duration::from_secs(1), schedulable!(Self::send), value).unwrap();
//! #         }
//! #         #[nexosim(schedulable)]
//! #         async fn send(&mut self, value: f64) { // this method can be private
//! #             self.output.send(value).await;
//! #         }
//! #     }
//! # }
//! # use std::time::Duration;
//! # use nexosim::ports::{EventSinkReader, EventSource, SinkState, event_slot};
//! # use nexosim::simulation::{Mailbox, SimInit};
//! # use nexosim::time::MonotonicTime;
//! # use models::{Delay, Multiplier};
//! # let mut multiplier1 = Multiplier::default();
//! # let mut multiplier2 = Multiplier::default();
//! # let mut delay1 = Delay::default();
//! # let mut delay2 = Delay::default();
//! # let multiplier1_mbox = Mailbox::new();
//! # let multiplier2_mbox = Mailbox::new();
//! # let delay1_mbox = Mailbox::new();
//! # let delay2_mbox = Mailbox::new();
//! # multiplier1.output.connect(Delay::input, &delay1_mbox);
//! # multiplier1.output.connect(Multiplier::input, &multiplier2_mbox);
//! # multiplier2.output.connect(Delay::input, &delay2_mbox);
//! # delay1.output.connect(Delay::input, &delay2_mbox);
//! # let mut bench = SimInit::new();
//! # let input = EventSource::new()
//! #     .connect(Multiplier::input, &multiplier1_mbox)
//! #     .register(&mut bench);
//! # let (sink, mut output) = event_slot(SinkState::Enabled);
//! # delay2.output.connect_sink(sink);
//! # let t0 = MonotonicTime::EPOCH;
//! # let mut simu = bench
//! #     .add_model(multiplier1, multiplier1_mbox, "multiplier1")
//! #     .add_model(multiplier2, multiplier2_mbox, "multiplier2")
//! #     .add_model(delay1, delay1_mbox, "delay1")
//! #     .add_model(delay2, delay2_mbox, "delay2")
//! #     .init(t0)?;
//! // Send a value to the first multiplier.
//! simu.process_event(&input, 21.0)?;
//!
//! // The simulation is still at t0 so nothing is expected at the output of the
//! // second delay gate.
//! assert!(output.try_read().is_none());
//!
//! // Advance simulation time until the next event and check the time and output.
//! simu.step()?;
//! assert_eq!(simu.time(), t0 + Duration::from_secs(1));
//! assert_eq!(output.try_read(), Some(84.0));
//!
//! // Get the answer to the ultimate question of life, the universe & everything.
//! simu.step()?;
//! assert_eq!(simu.time(), t0 + Duration::from_secs(2));
//! assert_eq!(output.try_read(), Some(42.0));
//!
//! # Ok::<(), nexosim::simulation::SimulationError>(())
//! ```
//!
//! # Message ordering guarantees
//!
//! The NeXosim runtime is based on the [actor model][actor_model], meaning that
//! every simulation model can be thought of as an isolated entity running in
//! its own thread. While in practice the runtime will actually multiplex and
//! migrate models over a fixed set of kernel threads, models will indeed run in
//! parallel whenever possible.
//!
//! Since NeXosim is a time-based simulator, the runtime will always execute
//! tasks in chronological order, thus eliminating most ordering ambiguities
//! that could result from parallel execution. Nevertheless, it is sometimes
//! possible for events and queries generated in the same time slice to lead to
//! ambiguous execution orders. In order to make it easier to reason about such
//! situations, NeXosim provides a set of guarantees about message delivery
//! order. Borrowing from the [Pony][pony] programming language, we refer to
//! this contract as *causal messaging*, a property that can be summarized by
//! these two rules:
//!
//! 1. *one-to-one message ordering guarantee*: if model `A` sends two events or
//!    queries `M1` and then `M2` to model `B`, then `B` will always process
//!    `M1` before `M2`,
//! 2. *transitivity guarantee*: if `A` sends `M1` to `B` and then `M2` to `C`
//!    which in turn sends `M3` to `B`, even though `M1` and `M2` may be
//!    processed in any order by `B` and `C`, it is guaranteed that `B` will
//!    process `M1` before `M3`.
//!
//! Both guarantees also extend to same-time events scheduled from the global
//! [`Scheduler`](simulation::Scheduler), *i.e.* the relative ordering of events
//! scheduled for the same time is preserved and warranties 1 and 2 above
//! accordingly hold (assuming model `A` stands for the scheduler). Likewise,
//! the relative order of same-time events self-scheduled by a model using its
//! [`Context`](model::Context) is preserved.
//!
//! [actor_model]: https://en.wikipedia.org/wiki/Actor_model
//! [pony]: https://www.ponylang.io/
//!
//!
//! # Cargo feature flags
//!
//! ## Tracing
//!
//! The `tracing` feature flag provides support for the
//! [`tracing`](https://docs.rs/tracing/latest/tracing/) crate and can be
//! activated in `Cargo.toml` with:
//!
//! ```toml
//! [dependencies]
//! nexosim = { version = "1", features = ["tracing"] }
//! ```
//!
//! See the [`tracing`] module for more information.
//!
//! ## Server
//!
//! The `server` feature provides a gRPC server for remote control and
//! monitoring, *e.g.* from a Python client. It can be activated with:
//!
//! ```toml
//! [dependencies]
//! nexosim = { version = "1", features = ["server"] }
//! ```
//!
//! See the [`endpoints`] and [`server`] modules for more information.
//!
//! Front-end usage documentation will be added upon release of the NeXosim
//! Python client.
//!
//!
//! # Other resources
//!
//! ## Other examples
//!
//! Several [`examples`][gh_examples] are available that contain more fleshed
//! out examples and demonstrate various capabilities of the simulation
//! framework.
//!
//! [gh_examples]:
//!     https://github.com/asynchronics/nexosim/tree/main/nexosim/examples
//!
//!
//! ## Other features and advanced topics
//!
//! While the above overview does cover most basic concepts, more information is
//! available in the modules' documentation:
//!
//! * the [`model`] module provides more details about models, **model
//!   prototypes** and **hierarchical models**; be sure to check as well the
//!   documentation of [`model::Context`] for topics such as **self-scheduling**
//!   methods and **event cancellation**,
//! * the [`ports`] module discusses in more details model ports and simulation
//!   endpoints, as well as the ability to **modify and filter messages**
//!   exchanged between ports; it also provides
//!   [`EventSource`](ports::EventSource) and
//!   [`QuerySource`](ports::QuerySource) objects which can be connected to
//!   models just like [`Output`](ports::Output) and
//!   [`Requestor`](ports::Requestor) ports, but for use as simulation
//!   endpoints.
//! * the [`server`] modules makes it possible to remotely manage a simulation
//!   bench via gRPC,
//! * the [`simulation`] module discusses mailbox capacity, deadlocks and custom
//!   clocks,
//! * the [`time`] module introduces [`MonotonicTime`](time::MonotonicTime)
//!   timestamps,  [`Clock`](time::Clock)s and [`Ticker`](time::Ticker)s.
//! * the [`tracing`] module discusses time-stamping and filtering of `tracing`
//!   events.
#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(docsrs, doc(auto_cfg(hide(feature = "dev-hooks"))))]

pub(crate) mod channel;
pub mod endpoints;
pub(crate) mod executor;
mod loom_exports;
pub(crate) mod macros;
pub mod model;
pub mod path;
pub mod ports;
#[cfg(feature = "server")]
pub mod server;
pub mod simulation;
pub mod time;
#[cfg(feature = "tracing")]
pub mod tracing;
pub(crate) mod util;

#[cfg(feature = "dev-hooks")]
#[doc(hidden)]
pub mod dev_hooks;

pub use nexosim_macros::{self, Message};
#[doc(hidden)]
pub use schemars::{self, JsonSchema};
