use crate::Message;
use crate::_alloc_prelude::*;

macro_rules! wrapper_impl {
    ($($desc:tt)+) => {
        forward_impl!(($($desc)+ where T: Message) => T);
    };
}

wrapper_impl!(<'a, T: ?Sized> Message for &'a T);
wrapper_impl!(<'a, T: ?Sized> Message for &'a mut T);
wrapper_impl!(<T: ?Sized> Message for Box<T>);
wrapper_impl!(<T: ?Sized> Message for alloc::rc::Rc<T>);
wrapper_impl!(<T: ?Sized> Message for alloc::rc::Weak<T>);
wrapper_impl!(<T: ?Sized> Message for alloc::sync::Arc<T>);
wrapper_impl!(<T: ?Sized> Message for alloc::sync::Weak<T>);
#[cfg(feature = "std")]
wrapper_impl!(<T: ?Sized> Message for std::sync::Mutex<T>);
#[cfg(feature = "std")]
wrapper_impl!(<T: ?Sized> Message for std::sync::RwLock<T>);
wrapper_impl!(<T: ?Sized> Message for core::cell::Cell<T>);
wrapper_impl!(<T: ?Sized> Message for core::cell::RefCell<T>);
wrapper_impl!(<'a, T: ?Sized + ToOwned> Message for alloc::borrow::Cow<'a, T>);
wrapper_impl!(<T> Message for core::num::Wrapping<T>);
wrapper_impl!(<T> Message for core::cmp::Reverse<T>);
