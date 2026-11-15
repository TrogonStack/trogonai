//! One dispatched server-sent event frame.

use crate::js_text::is_js_whitespace;

/// A frame the SSE wire parser dispatched: the `event`, `id` and `data` fields, each absent when the frame did not
/// carry it. `data` is the `data` lines joined with `\n`, and may be empty for a frame that held only a comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseFrame {
    /// The last `event` field.
    pub event: Option<String>,
    /// The last `id` field.
    pub id: Option<String>,
    /// The joined `data` fields.
    pub data: Option<String>,
}

impl SseFrame {
    /// Reads one frame block (the text between two blank lines). A block with no visible character, or with
    /// neither non-empty `data` nor a comment line, is no frame.
    pub(crate) fn parse(block: &str) -> Option<Self> {
        if block.chars().all(is_js_whitespace) {
            return None;
        }
        let mut frame = Self::default();
        let mut comment = false;
        for line in split_lines(block) {
            if line.starts_with(':') {
                comment = true;
                continue;
            }
            let Some((field, rest)) = line.split_once(':') else {
                continue;
            };
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            match field {
                "event" => frame.event = Some(value.to_owned()),
                "id" => frame.id = Some(value.to_owned()),
                "data" => {
                    frame.data = Some(match frame.data.take() {
                        Some(data) if !data.is_empty() => format!("{data}\n{value}"),
                        _ => value.to_owned(),
                    });
                }
                _ => {}
            }
        }
        (frame.data.as_deref().is_some_and(|data| !data.is_empty()) || comment).then_some(frame)
    }
}

/// The lines of `text` split at `\r\n`, `\r` and `\n`.
fn split_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => {
                lines.push(&text[start..at]);
                at += 1;
                start = at;
            }
            b'\r' => {
                lines.push(&text[start..at]);
                at += if bytes.get(at + 1) == Some(&b'\n') { 2 } else { 1 };
                start = at;
            }
            _ => at += 1,
        }
    }
    lines.push(&text[start..]);
    lines
}

#[cfg(test)]
#[path = "sse_frame_tests.rs"]
mod tests;
