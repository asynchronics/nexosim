use schemars::Message;

#[derive(Message)]
#[schemars(transform = "x")]
pub struct Struct;

fn main() {}
