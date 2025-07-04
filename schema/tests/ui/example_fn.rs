use schemars::Message;

#[derive(Message)]
#[schemars(example = "my_fn")]
pub struct Struct;

fn my_fn() {}

fn main() {}
