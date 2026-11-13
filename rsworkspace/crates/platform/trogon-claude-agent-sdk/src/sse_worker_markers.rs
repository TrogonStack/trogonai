//! The text shapes only the worker may produce, and the user-frame checks built on them.

use serde_json::Value;

use crate::{JsonObject, js_text::is_js_whitespace};

const PEER_NOTICE: &str = "Another Claude session sent a message";
const ARRIVED_PREFIX: &str = "A message arrived from ";
const WORKER_PREFIXES: [&str; 7] = [
    "<bash-stdout",
    "<bash-stderr",
    "<local-command-stdout",
    "<local-command-stderr",
    "<channel source=\"",
    "<teammate-message ",
    "<teammate-message>",
];
const NOTICE_PREFIXES: [&str; 3] = [
    "Another Claude session sent a message while you were working:\n",
    "Another Claude session sent a message:\n",
    "A peer session sent a message while you were working:\n",
];
const EMBEDDED_MARKERS: [&str; 8] = [
    "<bash-input>",
    "<command-message>",
    "<user-memory-input>",
    "<task-notification",
    "<mcp-resource-update",
    "<mcp-polling-update",
    "<fork-boilerplate>",
    "<local-command-caveat>",
];

/// Whether `text` opens with a `<cross-session-message ...>` tag: the tag name, then either `>` or a space or tab and
/// attribute text up to a `>` on the same line.
fn opens_cross_session_tag(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("<cross-session-message") else {
        return false;
    };
    if rest.starts_with('>') {
        return true;
    }
    let Some(attributes) = rest.strip_prefix([' ', '\t']) else {
        return false;
    };
    let ends_at = attributes.find(['>', '\r', '\n', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}']);
    ends_at.is_some_and(|index| attributes[index..].starts_with('>'))
}

/// The first notice prefix `text` starts with.
fn notice_prefix(text: &str) -> Option<&'static str> {
    NOTICE_PREFIXES.into_iter().find(|prefix| text.starts_with(prefix))
}

/// Whether `text` holds a `<tick>` element: an opening `<tick` followed by white space or `>` and a closing
/// `</tick>`, in any ASCII case.
fn holds_tick_element(text: &str) -> bool {
    let bytes = text.as_bytes();
    let opens = (0..bytes.len()).any(|at| {
        bytes[at..].len() >= 5
            && bytes[at] == b'<'
            && bytes[at + 1..at + 5].eq_ignore_ascii_case(b"tick")
            && text[at + 5..]
                .chars()
                .next()
                .is_some_and(|ch| ch == '>' || is_js_whitespace(ch))
    });
    opens && bytes.windows(7).any(|window| window.eq_ignore_ascii_case(b"</tick>"))
}

/// Whether text carries a marker only worker output contains.
pub(crate) fn is_worker_output_text(text: &str) -> bool {
    if WORKER_PREFIXES.iter().any(|prefix| text.starts_with(prefix)) {
        return true;
    }
    if opens_cross_session_tag(text)
        || notice_prefix(text).is_some_and(|prefix| opens_cross_session_tag(&text[prefix.len()..]))
    {
        return true;
    }
    if text.starts_with(ARRIVED_PREFIX) || text.starts_with(PEER_NOTICE) {
        let line_start = text.find('\n').map_or(0, |index| index + 1);
        if text[line_start..].starts_with('<') {
            return true;
        }
    }
    if notice_prefix(text).is_some_and(|prefix| text[prefix.len()..].starts_with('<')) {
        return true;
    }
    holds_tick_element(text) || EMBEDDED_MARKERS.iter().any(|marker| text.contains(marker))
}

/// The `message.content` of a payload, when `message` is an object that has one.
fn content(payload: &JsonObject) -> Option<&Value> {
    payload.get("message")?.get("content")
}

/// Whether a user payload has the shape of worker output: tool results, plugin prompts, or content that carries
/// a worker-only marker.
pub(crate) fn is_worker_output_user(payload: &JsonObject) -> bool {
    if payload.contains_key("tool_use_result") || payload.contains_key("plugin_prompt") {
        return true;
    }
    match content(payload) {
        Some(Value::String(text)) => is_worker_output_text(text),
        Some(Value::Array(items)) => items.iter().any(|item| {
            item.as_object().is_some_and(|object| match object.get("type") {
                None => false,
                Some(Value::String(kind)) if kind == "tool_result" => true,
                Some(Value::String(kind)) if kind == "text" => object
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(is_worker_output_text),
                Some(_) => false,
            })
        }),
        _ => false,
    }
}

/// Whether a user payload's content array holds an element that is not an object, or a `text` block without a
/// string `text`.
pub(crate) fn has_malformed_user_content(payload: &JsonObject) -> bool {
    let Some(Value::Array(items)) = content(payload) else {
        return false;
    };
    !items.iter().all(|item| match item {
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) != Some("text")
                || object.get("text").is_some_and(Value::is_string)
        }
        Value::Array(_) => true,
        _ => false,
    })
}

#[cfg(test)]
#[path = "sse_worker_markers_tests.rs"]
mod tests;
