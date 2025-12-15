//! Trait for model input and replier ports.

use std::future::{Future, Ready, ready};

use crate::model::{Context, Model};

use super::markers;

/// A function, method or closures that can be used as an *input port*.
///
/// This trait is in particular implemented for any function or method with the
/// following signature, where the futures returned by the `async` variants must
/// implement `Send`:
///
/// ```ignore
/// fn(&mut M) // argument elided, implies `T=()`
/// fn(&mut M, T)
/// fn(&mut M, T, &Context<M>)
/// fn(&mut M, T, &Context<M>, &mut M::Env)
/// async fn(&mut M) // argument elided, implies `T=()`
/// async fn(&mut M, T)
/// async fn(&mut M, T, &Context<M>)
/// async fn(&mut M, T, &Context<M>, &mut M::Env)
/// where
///     M: Model,
///     T: Clone + Send + 'static,
///     R: Send + 'static,
/// ```
#[diagnostic::on_unimplemented(
    message = "The trait `InputFn` is not implemented for {Self}",
    note = "Make sure the method has a mutable `self` reference."
)]
pub trait InputFn<'a, M: Model, T, S>: Send + 'static {
    /// The `Future` returned by the asynchronous method.
    type Future: Future<Output = ()> + Send + 'a;

    /// Calls the method.
    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        env: &'a mut M::Env,
    ) -> Self::Future;
}

impl<'a, M, F> InputFn<'a, M, (), markers::WithoutArguments> for F
where
    M: Model,
    F: FnOnce(&'a mut M) + Send + 'static,
{
    type Future = Ready<()>;

    fn call(
        self,
        model: &'a mut M,
        _arg: (),
        _cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model);

        ready(())
    }
}

impl<'a, M, T, F> InputFn<'a, M, T, markers::WithoutContext> for F
where
    M: Model,
    F: FnOnce(&'a mut M, T) + Send + 'static,
{
    type Future = Ready<()>;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        _cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg);

        ready(())
    }
}

impl<'a, M, T, F> InputFn<'a, M, T, markers::WithContext> for F
where
    M: Model,
    F: FnOnce(&'a mut M, T, &'a Context<M>) + Send + 'static,
{
    type Future = Ready<()>;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg, cx);

        ready(())
    }
}

impl<'a, M, T, F> InputFn<'a, M, T, markers::WithContextAndEnv> for F
where
    M: Model,
    F: FnOnce(&'a mut M, T, &'a Context<M>, &'a mut M::Env) + Send + 'static,
{
    type Future = Ready<()>;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg, cx, env);

        ready(())
    }
}

impl<'a, M, Fut, F> InputFn<'a, M, (), markers::AsyncWithoutArguments> for F
where
    M: Model,
    Fut: Future<Output = ()> + Send + 'a,
    F: FnOnce(&'a mut M) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        _arg: (),
        _cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model)
    }
}

impl<'a, M, T, Fut, F> InputFn<'a, M, T, markers::AsyncWithoutContext> for F
where
    M: Model,
    Fut: Future<Output = ()> + Send + 'a,
    F: FnOnce(&'a mut M, T) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        _cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg)
    }
}

impl<'a, M, T, Fut, F> InputFn<'a, M, T, markers::AsyncWithContext> for F
where
    M: Model,
    Fut: Future<Output = ()> + Send + 'a,
    F: FnOnce(&'a mut M, T, &'a Context<M>) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg, cx)
    }
}

impl<'a, M, T, Fut, F> InputFn<'a, M, T, markers::AsyncWithContextAndEnv> for F
where
    M: Model,
    Fut: Future<Output = ()> + Send + 'a,
    F: FnOnce(&'a mut M, T, &'a Context<M>, &'a mut M::Env) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg, cx, env)
    }
}

/// A function, method or closure that can be used as a *replier port*.
///
/// This trait is in particular implemented for any function or method with the
/// following signature, where the returned futures must implement `Send`:
///
/// ```ignore
/// async fn(&mut M) -> R // argument elided, implies `T=()`
/// async fn(&mut M, T) -> R
/// async fn(&mut M, T, &Context<M>) -> R
/// async fn(&mut M, T, &Context<M>, &mut M::Env) -> R
/// where
///     M: Model,
///     T: Clone + Send + 'static,
///     R: Send + 'static,
/// ```
#[diagnostic::on_unimplemented(
    message = "The trait `ReplierFn` is not implemented for {Self}",
    note = "Make sure the method has a mutable `self` reference and returns a future."
)]
pub trait ReplierFn<'a, M: Model, T, R, S>: Send + 'static {
    /// The `Future` returned by the asynchronous method.
    type Future: Future<Output = R> + Send + 'a;

    /// Calls the method.
    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        env: &'a mut M::Env,
    ) -> Self::Future;
}

impl<'a, M, R, Fut, F> ReplierFn<'a, M, (), R, markers::AsyncWithoutArguments> for F
where
    M: Model,
    Fut: Future<Output = R> + Send + 'a,
    F: FnOnce(&'a mut M) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        _arg: (),
        _cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model)
    }
}

impl<'a, M, T, R, Fut, F> ReplierFn<'a, M, T, R, markers::AsyncWithoutContext> for F
where
    M: Model,
    Fut: Future<Output = R> + Send + 'a,
    F: FnOnce(&'a mut M, T) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        _cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg)
    }
}

impl<'a, M, T, R, Fut, F> ReplierFn<'a, M, T, R, markers::AsyncWithContext> for F
where
    M: Model,
    Fut: Future<Output = R> + Send + 'a,
    F: FnOnce(&'a mut M, T, &'a Context<M>) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        _env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg, cx)
    }
}

impl<'a, M, T, R, Fut, F> ReplierFn<'a, M, T, R, markers::AsyncWithContextAndEnv> for F
where
    M: Model,
    Fut: Future<Output = R> + Send + 'a,
    F: FnOnce(&'a mut M, T, &'a Context<M>, &'a mut M::Env) -> Fut + Send + 'static,
{
    type Future = Fut;

    fn call(
        self,
        model: &'a mut M,
        arg: T,
        cx: &'a Context<M>,
        env: &'a mut M::Env,
    ) -> Self::Future {
        self(model, arg, cx, env)
    }
}
