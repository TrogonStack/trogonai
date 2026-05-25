//! Map ACP `ToolCallUpdate` notifications to CLI `StreamEvent::ToolFinished`.

use agent_client_protocol::{ToolCallContent, ToolCallStatus, ToolCallUpdate};
use serde_json::Value;

const EXIT_MARKER_PREFIX: &str = "__EXIT_";
const EXIT_MARKER_SUFFIX: &str = "__";

const OUTPUT_TRUNCATE: usize = 2048;

/// Parsed tool completion for terminal display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFinishedEvent {
    pub name: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub status: ToolCallStatus,
}

/// Returns `None` for in-progress or intermediate terminal streaming updates.
pub fn map_tool_call_update(update: &ToolCallUpdate) -> Option<ToolFinishedEvent> {
    if let Some(meta) = update.meta.as_ref() {
        if meta.contains_key("terminal_output") && !meta.contains_key("terminal_exit") {
            return None;
        }
    }

    let status = update.fields.status?;
    if status != ToolCallStatus::Completed && status != ToolCallStatus::Failed {
        return None;
    }

    let raw_output = extract_output(update.fields.raw_output.as_ref(), update.fields.content.as_deref());
    let output = truncate_output(strip_exit_markers(&raw_output));
    let exit_code = extract_exit_code(update, &raw_output, status);
    let name = tool_name_from_update(update);

    Some(ToolFinishedEvent {
        name,
        output,
        exit_code,
        status,
    })
}

fn tool_name_from_update(update: &ToolCallUpdate) -> String {
    if let Some(meta) = update.meta.as_ref() {
        if let Some(name) = meta.get("tool_name").and_then(|v| v.as_str()) {
            return name.to_string();
        }
    }
    update
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "tool".to_string())
}

fn extract_output(raw: Option<&Value>, _content: Option<&[ToolCallContent]>) -> String {
    if let Some(raw) = raw {
        if let Some(s) = raw.as_str() {
            if !s.is_empty() {
                return s.to_string();
            }
        }
        // Non-string JSON value (object/array) — serialize so the output is visible.
        if !raw.is_null() {
            let serialized = raw.to_string();
            if !serialized.is_empty() {
                return serialized;
            }
        }
    }
    String::new()
}

fn extract_exit_code(
    update: &ToolCallUpdate,
    raw_output: &str,
    status: ToolCallStatus,
) -> Option<i32> {
    if let Some(meta) = update.meta.as_ref() {
        if let Some(exit) = meta.get("terminal_exit") {
            if let Some(code) = exit.get("exit_code").and_then(|v| v.as_i64()) {
                return i32::try_from(code).ok();
            }
        }
    }
    if let Some(code) = parse_exit_marker(raw_output) {
        return Some(code);
    }
    match status {
        ToolCallStatus::Completed => Some(0),
        ToolCallStatus::Failed => None,
        _ => None,
    }
}

fn parse_exit_marker(output: &str) -> Option<i32> {
    let mut last: Option<i32> = None;
    let mut search = output;
    let mut offset = 0usize;
    while let Some(pos) = search.find(EXIT_MARKER_PREFIX) {
        let abs = offset + pos;
        let after = &output[abs + EXIT_MARKER_PREFIX.len()..];
        if let Some(end) = after.find(EXIT_MARKER_SUFFIX) {
            let code_str = &after[..end];
            if code_str.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(code) = code_str.parse::<i32>() {
                    last = Some(code);
                }
            }
        }
        offset = abs + EXIT_MARKER_PREFIX.len();
        search = &output[offset..];
    }
    last
}

fn strip_exit_markers(output: &str) -> String {
    if let Some(pos) = output.rfind(EXIT_MARKER_PREFIX) {
        let after = &output[pos + EXIT_MARKER_PREFIX.len()..];
        if let Some(end) = after.find(EXIT_MARKER_SUFFIX) {
            let code_str = &after[..end];
            if code_str.chars().all(|c| c.is_ascii_digit()) {
                let before = &output[..pos];
                return before.strip_suffix('\n').unwrap_or(before).to_string();
            }
        }
    }
    output.to_string()
}

fn truncate_output(mut output: String) -> String {
    if output.len() > OUTPUT_TRUNCATE {
        let boundary = output.floor_char_boundary(OUTPUT_TRUNCATE);
        output.truncate(boundary);
        output.push_str("… [truncated]");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::ToolCallId;
    use agent_client_protocol::ToolCallUpdateFields;

    #[test]
    fn terminal_output_only_update_is_skipped() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "terminal_output".to_string(),
            serde_json::json!({"terminal_id": "t1"}),
        );
        let update = ToolCallUpdate::new(
            ToolCallId::new("tc1"),
            ToolCallUpdateFields::new(),
        )
        .meta(meta);
        assert!(map_tool_call_update(&update).is_none());
    }

    #[test]
    fn terminal_exit_meta_yields_exit_code() {
        let mut exit = serde_json::Map::new();
        exit.insert("exit_code".to_string(), serde_json::json!(1));
        let mut meta = serde_json::Map::new();
        meta.insert("terminal_exit".to_string(), serde_json::Value::Object(exit));
        let update = ToolCallUpdate::new(
            ToolCallId::new("tc1"),
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .title("bash".to_string())
                .raw_output(serde_json::Value::String("error\n".to_string())),
        )
        .meta(meta);
        let ev = map_tool_call_update(&update).unwrap();
        assert_eq!(ev.exit_code, Some(1));
        assert_eq!(ev.name, "bash");
    }

    #[test]
    fn exit_marker_in_raw_output_parsed() {
        let update = ToolCallUpdate::new(
            ToolCallId::new("tc1"),
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .title("bash".to_string())
                .raw_output(serde_json::Value::String(
                    "hello\n__EXIT_1__\n".to_string(),
                )),
        );
        let ev = map_tool_call_update(&update).unwrap();
        assert_eq!(ev.exit_code, Some(1));
        assert_eq!(ev.output, "hello");
    }

    #[test]
    fn completed_without_code_infers_zero() {
        let update = ToolCallUpdate::new(
            ToolCallId::new("tc1"),
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .title("read_file".to_string())
                .raw_output(serde_json::Value::String("contents".to_string())),
        );
        let ev = map_tool_call_update(&update).unwrap();
        assert_eq!(ev.exit_code, Some(0));
    }

    #[test]
    fn truncate_output_multibyte_boundary_does_not_panic() {
        // CRIT-3: a multibyte UTF-8 char straddling OUTPUT_TRUNCATE must not panic.
        // "é" is 2 bytes and starts at odd offsets, so byte 2048 lands mid-char.
        let input = format!("a{}", "é".repeat(2000));
        assert!(input.len() > OUTPUT_TRUNCATE);
        let out = truncate_output(input);
        assert!(out.ends_with("… [truncated]"), "got: {out}");
        // Result is valid UTF-8 by construction (it's a String); the prefix must
        // have been cut on a char boundary.
        assert!(out.len() <= OUTPUT_TRUNCATE + "… [truncated]".len());
    }

    #[test]
    fn failed_has_no_exit_code() {
        let update = ToolCallUpdate::new(
            ToolCallId::new("tc1"),
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Failed)
                .title("bash".to_string())
                .raw_output(serde_json::Value::String("boom".to_string())),
        );
        let ev = map_tool_call_update(&update).unwrap();
        assert_eq!(ev.exit_code, None);
        assert_eq!(ev.status, ToolCallStatus::Failed);
    }
}
