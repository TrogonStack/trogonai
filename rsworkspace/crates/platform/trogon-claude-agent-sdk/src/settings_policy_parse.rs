//! What the managed settings reader of the TypeScript SDK (`lE(..., true)` upstream) makes of a document's text,
//! before it sanitizes the document.

use serde_json::{Map, Value};

use crate::{
    node_managed_pass::ManagedPass,
    node_stack::NodeStack,
    settings_parse::parse_json,
    settings_parse_deep::{nests_past_serde_json, parse_json_deep, parses_as_object},
    settings_strings::stand_in_escapes,
};

/// A managed settings document's text, read.
#[derive(Debug, PartialEq)]
pub(crate) enum PolicyParse {
    /// The document object, each lone surrogate held by its stand-in.
    Object(Map<String, Value>),
    /// `JSON.parse` rejects the text or reads something other than an object (`gE` upstream).
    NotObject,
    /// The reader runs out of stack copying or checking the object, which it reports as a file it could not read
    /// (`Q7e` upstream).
    Overflow,
}

/// Reads a managed settings document's text, not blank, as the reader does on the stack of `node` in `pass`:
/// `JSON.parse` at any depth after a leading byte order mark, then the copy and the schema check, which run out of
/// stack on a document nested too deep.
pub(crate) fn parse_policy_text(text: &str, node: NodeStack, pass: ManagedPass) -> PolicyParse {
    let text = stand_in_escapes(text.strip_prefix('\u{feff}').unwrap_or(text));
    let parsed = if nests_past_serde_json(&text) {
        match parse_json_deep(&text, node.policy_max_depth(pass)) {
            Some(parsed) => parsed,
            None if parses_as_object(&text) == Some(true) => return PolicyParse::Overflow,
            None => return PolicyParse::NotObject,
        }
    } else {
        match parse_json(&text) {
            Ok(parsed) => parsed,
            Err(_) => return PolicyParse::NotObject,
        }
    };
    let Value::Object(document) = parsed else {
        return PolicyParse::NotObject;
    };
    if node.reads_policy(pass, &document) {
        PolicyParse::Object(document)
    } else {
        PolicyParse::Overflow
    }
}

#[cfg(test)]
#[path = "settings_policy_parse_tests.rs"]
mod tests;
