#![no_std]

use schemars::{schema_for, Message};

#[derive(Message, Default)]
pub struct MyStruct {
    /// A number
    pub my_int: i32,
    #[schemars(extend("x-test" = {"k": "v"}))]
    pub my_bool: bool,
    pub my_nullable_enum: Option<MyEnum>,
}

#[derive(Message)]
pub enum MyEnum {
    StringNewType(&'static str),
    StructVariant { floats: &'static [f32] },
}

#[test]
fn test_no_std() {
    let _schema = schema_for!(MyStruct);
}
