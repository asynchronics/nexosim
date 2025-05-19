#[macro_export]
macro_rules! schedulable {
    ($func:ident) => {
        nexosim::paste! { Self::[<__ $func>]() }
    };
}
