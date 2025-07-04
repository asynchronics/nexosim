use crate::SchemaGenerator;
use crate::{json_schema, Message, Schema};
use alloc::borrow::Cow;
use url2::Url;

impl Message for Url {
    inline_schema!();

    fn schema_name() -> Cow<'static, str> {
        "Url".into()
    }

    fn schema_id() -> Cow<'static, str> {
        "url::Url".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "uri",
        })
    }
}
