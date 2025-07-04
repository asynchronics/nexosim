use schemars::Message;

struct DoesNotImplMessage;

#[derive(Message)]
#[schemars(rename = "} } {T} {U} {T::test} } {T")]
pub struct Struct1<T> {
    pub t: T,
}

fn main() {}
