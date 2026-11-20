//! What the managed settings reader of the TypeScript SDK (`lE(..., true)` upstream) makes of a document's text,
//! before it sanitizes the document.

use serde_json::{Map, Value};

use crate::{
    constants::SETTINGS_DEPTH_BOUND,
    settings_depth::check_text,
    settings_parse::parse_json,
    settings_parse_deep::{nests_past_serde_json, parse_json_deep},
    settings_resolve_error::ResolveSettingsError,
    settings_strings::stand_in_escapes,
};

/// Reads a managed settings document's text, not blank: `JSON.parse` after a leading byte order mark, at any depth below
/// [`SETTINGS_DEPTH_BOUND`](crate::SETTINGS_DEPTH_BOUND). The document object has each lone surrogate held by its
/// stand-in. `None` when `JSON.parse` rejects the text or reads something other than an object (`gE` upstream). An object
/// document nested at the bound or deeper fails with [`ResolveSettingsError::MaximumCallStack`].
pub(crate) fn parse_policy_text(text: &str) -> Result<Option<Map<String, Value>>, ResolveSettingsError> {
    check_text(text)?;
    let text = stand_in_escapes(text.strip_prefix('\u{feff}').unwrap_or(text));
    let parsed = if nests_past_serde_json(&text) {
        parse_json_deep(&text, SETTINGS_DEPTH_BOUND)
    } else {
        parse_json(&text).ok()
    };
    Ok(match parsed {
        Some(Value::Object(document)) => Some(document),
        _ => None,
    })
}

#[cfg(test)]
#[path = "settings_policy_parse_tests.rs"]
mod tests;
