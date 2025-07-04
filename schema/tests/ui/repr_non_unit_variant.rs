use schemars::Message_repr;

#[derive(Message_repr)]
#[repr(u8)]
pub enum Enum {
    Unit,
    EmptyTuple(),
}

fn main() {}
