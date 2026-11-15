#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests build credential blocks as the JSON a managed document holds."
    )
)]

use serde_json::json;

use super::*;

#[test]
fn file_prefix_hashes_the_file_name_like_upstream() {
    assert_eq!(file_prefix(None), "");
    assert_eq!(file_prefix(Some("f")), "E30C2799_");
    assert_eq!(file_prefix(Some("a😀b")), "FA52C473_");
    assert_eq!(file_prefix(Some("/etc/claude-code/managed-settings.json")), "5F05A873_");
}

#[test]
fn a_schema_without_the_credential_lists_reads_nothing() {
    assert!(element_of(&Node::Boolean).is_none());
    assert_eq!(all_denied(&Node::Boolean), json!({}));
    assert!(read(&Node::Boolean, None, &json!({})).is_none());
}
