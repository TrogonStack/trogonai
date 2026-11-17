use super::{PolicyParse, parse_policy_text};
use crate::{node_managed_pass::ManagedPass, node_stack::NodeStack};

fn read(text: &str) -> PolicyParse {
    parse_policy_text(text, NodeStack::NODE_24_DARWIN_ARM64, ManagedPass::Sources)
}

#[test]
fn a_document_that_parses_to_something_other_than_an_object_is_not_an_object() {
    assert_eq!(read("[]"), PolicyParse::NotObject);
    assert_eq!(read("\u{feff}\"text\""), PolicyParse::NotObject);
    let arrays = format!("{}{}", "[".repeat(200), "]".repeat(200));
    assert_eq!(read(&arrays), PolicyParse::NotObject);
}

#[test]
fn a_document_nested_past_serde_json_that_json_parse_rejects_is_not_an_object() {
    assert_eq!(read(&"[".repeat(200)), PolicyParse::NotObject);
}

#[test]
fn a_document_nested_past_serde_json_reads_as_its_object() {
    let text = format!("{}null{}", "{\"x\":".repeat(200), "}".repeat(200));
    assert!(matches!(read(&text), PolicyParse::Object(document) if document.keys().eq(["x"])));
}
