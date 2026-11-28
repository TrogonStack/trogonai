use trogon_peer_context::escape_context_json;

use crate::constants::MAX_QUOTED_CHARS;

/// A caller- or peer-chosen string as a bounded JSON string literal that cannot open markup.
pub(crate) fn quoted(value: &str) -> String {
    let bounded: String = value.chars().take(MAX_QUOTED_CHARS).collect();
    escape_context_json(&serde_json::Value::String(bounded).to_string())
}
