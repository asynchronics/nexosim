//! Ports for event and query broadcasting.
//!
//!
//! # Events and queries
//!
//! Models can exchange data via *events* and *queries*.
//!
//! Events are send-and-forget messages that can be broadcast from an [`Output`]
//! port or [`EventSource`] to an arbitrary number of *input ports* or
//! [`EventSink`]s with a matching event type.
//!
//! Queries actually involve two messages: a *request* that can be broadcast
//! from a [`Requestor`] port, a [`UniRequestor`] port or a [`QuerySource`] to
//! an arbitrary number of *replier ports* with a matching request type, and a
//! *reply* sent in response to such request. The response received by a
//! [`Requestor`] port is an iterator that yields as many items (replies) as
//! there are connected replier ports, while a [`UniRequestor`] received exactly
//! one reply.
//!
//! # Model ports
//!
//! ## Input and replier ports
//!
//! Input ports and replier ports are methods that implement the [`InputFn`] or
//! [`ReplierFn`] traits.
//!
//! In practice, an input port method for an event of type `T` may have any of
//! the following signatures, where the futures returned by the `async` variants
//! must implement `Send`:
//!
//! ```ignore
//! fn(&mut self) // argument elided, implies `T=()`
//! fn(&mut self, T)
//! fn(&mut self, T, &Context<Self>)
//! fn(&mut self, T, &Context<Self>, &mut Self::Env)
//! async fn(&mut self) // argument elided, implies `T=()`
//! async fn(&mut self, T)
//! async fn(&mut self, T, &Context<Self>)
//! async fn(&mut self, T, &Context<Self>, &mut Self::Env)
//! where
//!     Self: Model,
//!     T: Clone + Send + 'static,
//!     R: Send + 'static,
//! ```
//!
//! The context argument is useful for methods that need access to the
//! simulation time or that need to schedule an action at a future date.
//!
//! A replier port for a request of type `T` with a reply of type `R` may in
//! turn have any of the following signatures, where the futures must implement
//! `Send`:
//!
//! ```ignore
//! async fn(&mut self) -> R // argument elided, implies `T=()`
//! async fn(&mut self, T) -> R
//! async fn(&mut self, T, &Context<Self>) -> R
//! async fn(&mut self, T, &Context<Self>, &mut Self::Env) -> R
//! where
//!     Self: Model,
//!     T: Clone + Send + 'static,
//!     R: Send + 'static,
//! ```
//!
//! Note that, due to type resolution ambiguities, non-async methods are not
//! allowed for replier ports.
//!
//! Input and replier ports will normally be exposed as public methods by a
//! [`Model`](trait@crate::model::Model) so they can be connected to
//! output and requestor ports when assembling the simulation bench. However,
//! input ports may (and should) be defined as private methods if they are only
//! used internally by the model, for instance to schedule future actions on
//! itself.
//!
//! Changing the signature of a public input or replier port is not considered
//! to alter the public interface of a model provided that the event, request
//! and reply types remain the same. In particular, adding a context argument or
//! changing a regular method to an `async` method will not cause idiomatic user
//! code to miscompile.
//!
//! #### Basic example
//!
//! ```
//! use nexosim::model::{Model, Context};
//!
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize)]
//! pub struct MyModel {
//!     // ...
//! }
//! #[Model]
//! impl MyModel {
//!     pub fn my_input(&mut self, input: String, cx: &Context<Self>) {
//!         // ...
//!     }
//!     pub async fn my_replier(&mut self, request: u32) -> bool { // context argument elided
//!         // ...
//!         # unimplemented!()
//!     }
//! }
//! ```
//!
//! ## Output and requestor ports
//!
//! Output and requestor ports can be added to a model using composition, adding
//! [`Output`], [`Requestor`] or [`UniRequestor`] objects as members. They are
//! parametrized by the event type, or by the request and reply types.
//!
//! Output ports broadcast events to all connected input ports, while requestor
//! ports broadcast queries to, and retrieve replies from, all connected replier
//! ports.
//!
//! On the surface, output and requestor ports only differ in that sending a
//! query from a requestor port also returns an iterator over the replies from
//! all connected ports (or a single reply in the case of [`UniRequestor`]).
//! Sending a query is more costly, however, because of the need to wait until
//! the connected model(s) have processed the query. In contrast, since events
//! are buffered in the mailbox of the target model, sending an event is a
//! fire-and-forget operation. For this reason, output ports should generally be
//! preferred over requestor ports when possible.
//!
//! Models (or model prototypes, as appropriate) are expected to expose their
//! output and requestor ports as public members so they can be connected to
//! input and replier ports when assembling the simulation bench. Internal ports
//! used by hierarchical models to communicate with submodels are an exception
//! to this rule and are typically private.
//!
//! #### Basic example
//!
//! ```
//! use nexosim::model::Model;
//! use nexosim::ports::{Output, Requestor};
//!
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize)]
//! pub struct MyModel {
//!     pub my_output: Output<String>,
//!     pub my_requestor: Requestor<u32, bool>,
//! }
//! #[Model]
//! impl MyModel {
//!     // ...
//! }
//! ```
//!
//! #### Example with cloned ports
//!
//! [`Output`] and [`Requestor`] ports are cloneable. The clones are shallow
//! copies, meaning that any modification of the ports connected to one instance
//! is immediately reflected by its clones.
//!
//! Clones of output and requestor ports should be used with care: even though
//! they uphold the usual [ordering
//! guaranties](crate#message-ordering-guarantees), their use can lead to
//! somewhat surprising message orderings.
//!
//! For instance, in the below [hierarchical
//! model](crate::model#hierarchical-models), while message `M0` is guaranteed
//! to reach the cloned output first, the relative arrival order of messages
//! `M1` and `M2` forwarded by the submodels to the cloned output is not
//! guaranteed.
//!
//! ```
//! use nexosim::model::{BuildContext, Model, ProtoModel};
//! use nexosim::ports::Output;
//! use nexosim::simulation::Mailbox;
//!
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize)]
//! pub struct ParentModel {
//!     output: Output<String>,
//!     to_child1: Output<String>,
//!     to_child2: Output<String>,
//! }
//! #[Model]
//! impl ParentModel {
//!     pub async fn trigger(&mut self) {
//!         // M0 is guaranteed to reach the cloned output first.
//!         self.output.send("M0".to_string()).await;
//!
//!         // M1 and M2 are forwarded by `Child1` and `Child2` to the cloned output
//!         // but may reach it in any relative order.
//!         self.to_child1.send("M1".to_string()).await;
//!         self.to_child2.send("M2".to_string()).await;
//!     }
//! }
//!
//! pub struct ProtoParentModel {
//!     pub output: Output<String>,
//! }
//! impl ProtoParentModel {
//!     pub fn new() -> Self {
//!         Self {
//!             output: Default::default(),
//!         }
//!     }
//! }
//! impl ProtoModel for ProtoParentModel {
//!     type Model = ParentModel;
//!     fn build(self, cx: &mut BuildContext<Self>) -> (ParentModel, ()) {
//!         let child1 = ChildModel { output: self.output.clone() };
//!         let child2 = ChildModel { output: self.output.clone() };
//!         let mut parent = ParentModel {
//!             output: self.output,
//!             to_child1: Output::default(),
//!             to_child2: Output::default(),
//!         };
//!
//!         let child1_mailbox = Mailbox::new();
//!         let child2_mailbox = Mailbox::new();
//!         parent
//!             .to_child1
//!             .connect(ChildModel::input, child1_mailbox.address());
//!         parent
//!             .to_child2
//!             .connect(ChildModel::input, child2_mailbox.address());
//!
//!         cx.add_submodel(child1, child1_mailbox, "child1");
//!         cx.add_submodel(child2, child2_mailbox, "child2");
//!
//!         (parent, ())
//!     }
//! }
//!
//! #[derive(Serialize, Deserialize)]
//! pub struct ChildModel {
//!     pub output: Output<String>,
//! }
//! #[Model]
//! impl ChildModel {
//!     async fn input(&mut self, msg: String) {
//!         self.output.send(msg).await;
//!     }
//! }
//! ```
//!
//! # Simulation endpoints
//!
//! Simulation endpoints can be seen as entry and exit ports for a simulation
//! bench.
//!
//! [`EventSource`] and [`QuerySource`] objects are similar to [`Output`] and
//! [`Requestor`] ports, respectively. They can be connected to models and can
//! be used to send events or queries to such models via
//! [`Action`](crate::simulation::Action)s.
//!
//! Objects implementing the [`EventSink`] trait, such as [`EventSlot`] and
//! [`EventQueue`], are in turn similar to input ports. They can be connected to
//! model outputs and collect events sent by such models.
//!
//!
//! # Connections
//!
//! Model ports can be connected to other model ports and to simulation
//! endpoints using the `*connect` family of methods exposed by [`Output`],
//! [`Requestor`], [`UniRequestor`], [`EventSource`] and [`QuerySource`].
//!
//! Regular connections between two ports are made with the appropriate
//! `connect` method (for instance [`Output::connect`]). Such connections
//! broadcast events or requests from a sender port to one or several receiver
//! ports, cloning the event or request if necessary.
//!
//! Sometimes, however, it may be necessary to also map the event or request to
//! another type so it can be understood by the receiving port. While this
//! translation could be done by a model placed between the two ports, the
//! `map_connect` methods (for instance [`Output::map_connect`]) provide a
//! lightweight and computationally efficient alternative. These methods take a
//! mapping closure as argument which maps outgoing messages, and in the case of
//! requestors, a mapping closure which maps replies.
//!
//! Finally, it is sometime necessary to only forward messages or requests that
//! satisfy specific criteria. For instance, a model of a data bus may be
//! connected to many models (the "peripherals"), but its messages are usually
//! only addressed to selected models. The `filter_map_connect` methods (for
//! instance [`Output::filter_map_connect`]) enable this use-case by accepting a
//! closure that inspects the messages and determines whether they should be
//! forwarded, possibly after being mapped to another type.
mod input;
mod output;
mod sink;
mod source;

pub use input::markers;
pub use input::{InputFn, ReplierFn};
pub use output::{Output, Requestor, UniRequestor};
#[allow(deprecated)]
pub use sink::{
    blocking_event_queue::{BlockingEventQueue, BlockingEventQueueReader},
    event_buffer::EventBuffer,
    EventSinkStream,
};
pub use sink::{
    event_queue::{EventQueue, EventQueueReader},
    event_slot::EventSlot,
    EventSink, EventSinkReader, EventSinkWriter,
};
pub use source::{EventSource, QuerySource, ReplyReceiver};

pub(crate) use output::PORT_REG;
pub(crate) use source::{RegisteredEventSource, RegisteredQuerySource, ReplyIterator};
