//! Model components.
//!
//! # Models and model prototypes
//!
//! Every model must implement the [`Model`] trait. This trait defines an
//! asynchronous initialization method, [`Model::init`], which main purpose is
//! to enable models to perform specific actions when the simulation starts,
//! *i.e.* after all models have been connected and added to the simulation.
//!
//! It is frequently convenient to expose to users a model builder type—called a
//! *model prototype*—rather than the final model. This can be done by
//! implementing the [`ProtoModel`] trait, which defines the associated model
//! type and a [`ProtoModel::build`] method invoked when a model is added the
//! simulation.
//!
//! Prototype models can be used whenever the Rust builder pattern is helpful,
//! for instance to set optional parameters. One of the use-cases that may
//! benefit from the use of prototype models is hierarchical model building.
//! When a parent model contains submodels, these submodels are often an
//! implementation detail that needs not be exposed to the user. One may then
//! define a prototype model that contains all outputs and requestors ports,
//! while the model itself contains the input and replier ports. Upon invocation
//! of [`ProtoModel::build`], the exit ports are moved to the model or its
//! submodels, and those submodels are added to the simulation.
//!
//! Note that a trivial [`ProtoModel`] implementation is generated by default
//! for any object implementing the [`Model`] trait, where the associated
//! [`ProtoModel::Model`] type is the model type itself. This is what makes it
//! possible to use either an explicitly-defined [`ProtoModel`] as argument to
//! the [`SimInit::add_model`](crate::simulation::SimInit::add_model) method, or
//! a plain [`Model`] type.
//!
//! #### Examples
//!
//! A model that does not require initialization or building can simply use the
//! default implementation of the [`Model`] trait:
//!
//! ```
//! use nexosim::model::Model;
//!
//! pub struct MyModel {
//!     // ...
//! }
//! impl Model for MyModel {}
//! ```
//!
//! If a default action is required during simulation initialization, the `init`
//! methods can be explicitly implemented:
//!
//! ```
//! use nexosim::model::{Context, InitializedModel, Model};
//!
//! pub struct MyModel {
//!     // ...
//! }
//! impl Model for MyModel {
//!     async fn init(
//!         mut self,
//!         ctx: &mut Context<Self>
//!     ) -> InitializedModel<Self> {
//!         println!("...initialization...");
//!
//!         self.into()
//!     }
//! }
//! ```
//!
//! Finally, if a model builder is required, the [`ProtoModel`] trait can be
//! explicitly implemented. Note that the [`ProtoModel`] contains all output and
//! requestor ports, while the associated [`Model`] contains all input and
//! replier methods.
//!
//! ```
//! use nexosim::model::{BuildContext, InitializedModel, Model, ProtoModel};
//! use nexosim::ports::Output;
//!
//! /// The final model.
//! pub struct Multiplier {
//!     // Private outputs and requestors stored in a form that constitutes an
//!     // implementation detail and should not be exposed to the user.
//!     my_outputs: Vec<Output<usize>>
//! }
//! impl Multiplier {
//!     // Private constructor: the final model is built by the prototype model.
//!     fn new(
//!         value_times_1: Output<usize>,
//!         value_times_2: Output<usize>,
//!         value_times_3: Output<usize>,
//!     ) -> Self {
//!         Self {
//!             my_outputs: vec![value_times_1, value_times_2, value_times_3]
//!         }
//!     }
//!
//!     // Public input to be used during bench construction.
//!     pub async fn my_input(&mut self, my_data: usize) {
//!         for (i, output) in self.my_outputs.iter_mut().enumerate() {
//!             output.send(my_data*(i + 1)).await;
//!         }
//!     }
//! }
//! impl Model for Multiplier {}
//!
//! pub struct ProtoMultiplier {
//!     // Prettyfied outputs exposed to the user.
//!     pub value_times_1: Output<usize>,
//!     pub value_times_2: Output<usize>,
//!     pub value_times_3: Output<usize>,
//! }
//! impl ProtoModel for ProtoMultiplier {
//!     type Model = Multiplier;
//!
//!     fn build(
//!         mut self,
//!         _: &mut BuildContext<Self>
//!     ) -> Multiplier {
//!         Multiplier::new(self.value_times_1, self.value_times_2, self.value_times_3)
//!     }
//! }
//! ```
//!
//! # Hierarchical models
//!
//! Hierarchical models are models build from a prototype, which prototype adds
//! submodels to the simulation within its [`ProtoModel::build`] method. From a
//! formal point of view, however, hierarchical models are just regular models
//! implementing the [`Model`] trait, as are their submodels.
//!
//!
//! #### Example
//!
//! This example demonstrates a child model inside a parent model, where the
//! parent model simply forwards input data to the child and the child in turn
//! sends the data to the output exposed by the parent's prototype.
//!
//! For a more comprehensive example demonstrating hierarchical model
//! assemblies, see the [`assembly`][assembly] example.
//!
//! [assembly]:
//!     https://github.com/asynchronics/nexosim/tree/main/nexosim/examples/assembly.rs
//!
//! ```
//! use nexosim::model::{BuildContext, Model, ProtoModel};
//! use nexosim::ports::Output;
//! use nexosim::simulation::Mailbox;
//!
//! pub struct ParentModel {
//!     // Private internal port connected to the submodel.
//!     to_child: Output<u64>,
//! }
//! impl ParentModel {
//!     async fn input(&mut self, my_data: u64) {
//!         // Forward to the submodel.
//!         self.to_child.send(my_data).await;
//!     }
//! }
//! impl Model for ParentModel {}
//!
//! pub struct ProtoParentModel {
//!     pub output: Output<u64>,
//! }
//! impl ProtoModel for ProtoParentModel {
//!     type Model = ParentModel;
//!
//!     fn build(self, cx: &mut BuildContext<Self>) -> ParentModel {
//!         // Move the output to the child model.
//!         let child = ChildModel { output: self.output };
//!         let mut parent = ParentModel {
//!             to_child: Output::default(),
//!         };
//!
//!         let child_mailbox = Mailbox::new();
//!
//!         // Establish an internal Parent -> Child connection.
//!         parent
//!             .to_child
//!             .connect(ChildModel::input, child_mailbox.address());
//!
//!         // Add the child model to the simulation.
//!         cx.add_submodel(child, child_mailbox, "child");
//!
//!         parent
//!     }
//! }
//!
//! struct ChildModel {
//!     output: Output<u64>,
//! }
//! impl ChildModel {
//!     async fn input(&mut self, my_data: u64) {
//!         self.output.send(my_data).await;
//!     }
//! }
//! impl Model for ChildModel {}
//!
//! ```
use std::future::Future;

pub use context::{BuildContext, Context};

mod context;

/// Trait to be implemented by simulation models.
///
/// This trait enables models to perform specific actions during initialization.
/// The [`Model::init`] method is run only once all models have been connected
/// and migrated to the simulation bench, but before the simulation actually
/// starts. A common use for `init` is to send messages to connected models at
/// the beginning of the simulation.
///
/// The `init` function converts the model to the opaque `InitializedModel` type
/// to prevent an already initialized model from being added to the simulation
/// bench.
pub trait Model: Sized + Send + 'static {
    /// Performs asynchronous model initialization.
    ///
    /// This asynchronous method is executed exactly once for all models of the
    /// simulation when the
    /// [`SimInit::init`](crate::simulation::SimInit::init) method is called.
    ///
    /// The default implementation simply converts the model to an
    /// `InitializedModel` without any side effect.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::future::Future;
    /// use std::pin::Pin;
    ///
    /// use nexosim::model::{Context, InitializedModel, Model};
    ///
    /// pub struct MyModel {
    ///     // ...
    /// }
    ///
    /// impl Model for MyModel {
    ///     async fn init(
    ///         self,
    ///         cx: &mut Context<Self>
    ///     ) -> InitializedModel<Self> {
    ///         println!("...initialization...");
    ///
    ///         self.into()
    ///     }
    /// }
    /// ```
    fn init(self, _: &mut Context<Self>) -> impl Future<Output = InitializedModel<Self>> + Send {
        async { self.into() }
    }
}

/// Opaque type containing an initialized model.
///
/// A model can be converted to an `InitializedModel` using the `Into`/`From`
/// traits. The implementation of the simulation guarantees that the
/// [`Model::init`] method will never be called on a model after conversion to
/// an `InitializedModel`.
#[derive(Debug)]
pub struct InitializedModel<M: Model>(pub(crate) M);

impl<M: Model> From<M> for InitializedModel<M> {
    fn from(model: M) -> Self {
        InitializedModel(model)
    }
}

/// Trait to be implemented by simulation model prototypes.
///
/// This trait makes it possible to build the final model from a builder type
/// when it is added to the simulation.
///
/// The [`ProtoModel::build`] method consumes the prototype. It is
/// automatically called when a model or submodel prototype is added to the
/// simulation using
/// [`Simulation::add_model`](crate::simulation::SimInit::add_model) or
/// [`BuildContext::add_submodel`].
pub trait ProtoModel: Sized {
    /// Type of the model to be built.
    type Model: Model;

    /// Builds the model.
    ///
    /// This method is invoked when the
    /// [`SimInit::add_model`](crate::simulation::SimInit::add_model) or
    /// [`BuildContext::add_submodel`] method are called.
    fn build(self, cx: &mut BuildContext<Self>) -> Self::Model;
}

// Every model can be used as a prototype for itself.
impl<M: Model> ProtoModel for M {
    type Model = Self;

    fn build(self, _: &mut BuildContext<Self>) -> Self::Model {
        self
    }
}
