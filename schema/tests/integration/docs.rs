use crate::prelude::*;

#[allow(dead_code)]
#[derive(Message)]
/**
# This is the struct's title

This is the struct's description.
*/
struct MyStruct {
    /// # An integer
    my_int: i32,
    my_undocumented_bool: bool,
    /// A unit struct instance
    my_unit: MyUnitStruct,
    #[doc = concat!("# Documented ", "bool")]
    #[doc = concat!("This bool is documented")]
    my_documented_bool: bool,
}

/// # A Unit
#[derive(Message)]
struct MyUnitStruct;

#[test]
fn doc_comments_struct() {
    test!(MyStruct).assert_snapshot();
}

#[allow(dead_code)]
#[doc = " # This is the enum's title "]
#[doc = " This is "]
#[derive(Message)]
#[doc = " the enum's description."]
enum MyEnum {
    UndocumentedUnit,
    UndocumentedUnit2,
    /// This comment is included in the generated schema :)
    DocumentedUnit,
    /// ## Complex variant
    /// This is a struct-like variant.
    Complex {
        /// ### A nullable string
        ///
        /// This field is a nullable string.
        ///
        /// This
        ///is
        ///   the second
        ///  line!
        ///
        ///
        ///
        ///
        /// And this is the third!
        my_nullable_string: Option<String>,
    },
}

#[test]
fn doc_comments_enum() {
    test!(MyEnum).assert_snapshot();
}

/// # OverrideDocs struct
/// This description should be overridden
#[allow(dead_code)]
#[derive(Message)]
#[schemars(description = "New description")]
struct OverrideDocs {
    /// # Overridden
    #[schemars(title = "My integer", description = "This is an i32")]
    my_int: i32,
    /// # Overridden
    /// Also overridden
    #[schemars(title = "", description = "")]
    my_undocumented_bool: bool,
    #[schemars(title = concat!("Documented ", "bool"), description = "Capitalized".to_uppercase())]
    my_documented_bool: bool,
}

#[test]
fn doc_comments_override() {
    test!(OverrideDocs).assert_snapshot();
}
