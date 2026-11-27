use serde_json::json;

use super::*;

#[test]
fn members_become_an_object() {
    assert_eq!(
        object([("a", Value::from(1)), ("b", Value::from("x"))]),
        json!({"a": 1, "b": "x"})
    );
    assert_eq!(object([]), json!({}));
}
