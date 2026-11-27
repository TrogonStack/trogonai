use std::collections::BTreeMap;

use super::json_reply;
use crate::tool_error::ToolError;

#[test]
fn a_serializable_value_becomes_json() -> Result<(), ToolError> {
    let reply = json_reply(&BTreeMap::from([("a", 1)]))?;
    assert_eq!(reply.0["a"], 1);
    Ok(())
}

#[test]
fn a_value_that_cannot_be_encoded_is_an_encode_error() {
    let unencodable = BTreeMap::from([(vec![1u8], 1)]);
    assert!(matches!(json_reply(&unencodable), Err(ToolError::Encode(_))));
}
