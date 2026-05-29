//! Styled tool-permission prompts for the unified CLI UI.

use agent_client_protocol::RequestPermissionRequest;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDisplay {
    pub tool_name: String,
    pub detail: String,
}

pub fn permission_from_request(req: &RequestPermissionRequest) -> PermissionDisplay {
    let tool_name = req
        .tool_call
        .fields
        .title
        .as_deref()
        .unwrap_or("tool")
        .to_string();
    let detail = req
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .map(detail_from_value)
        .unwrap_or_default();
    PermissionDisplay { tool_name, detail }
}

fn detail_from_value(v: &Value) -> String {
    if let Some(cmd) = v.get("command").and_then(Value::as_str) {
        return cmd.to_string();
    }
    if let Some(path) = v.get("path").and_then(Value::as_str) {
        let mut lines = vec![path.to_string()];
        if let Some(old) = v.get("old_string").and_then(Value::as_str) {
            lines.push(format!("  − {}", truncate_line(old, 120)));
        }
        if let Some(new) = v.get("new_string").and_then(Value::as_str) {
            lines.push(format!("  + {}", truncate_line(new, 120)));
        }
        return lines.join("\n");
    }
    if let Some(url) = v.get("url").and_then(Value::as_str) {
        return url.to_string();
    }
    if let Some(query) = v.get("query").and_then(Value::as_str) {
        return query.to_string();
    }
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    truncate_line(&v.to_string(), 160)
}

fn truncate_line(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

pub fn print_permission_prompt(display: &PermissionDisplay) {
    eprintln!();
    eprintln!(
        "\x1b[1;33m▸ permission\x1b[0m  \x1b[1m{}\x1b[0m",
        display.tool_name
    );
    if !display.detail.is_empty() {
        for line in display.detail.lines() {
            eprintln!("\x1b[90m  {line}\x1b[0m");
        }
    }
    eprintln!();
    eprintln!("  \x1b[32m[a]\x1b[0m allow   \x1b[33m[w]\x1b[0m always allow   \x1b[31m[r]\x1b[0m reject");
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{ToolCallId, ToolCallUpdate, ToolCallUpdateFields};

    fn req(title: &str, raw: Value) -> RequestPermissionRequest {
        let mut fields = ToolCallUpdateFields::new();
        fields.title = Some(title.into());
        fields.raw_input = Some(raw);
        RequestPermissionRequest::new(
            "sess-1",
            ToolCallUpdate::new(ToolCallId::new("tc-1"), fields),
            vec![],
        )
    }

    #[test]
    fn bash_command_extracted() {
        let d = permission_from_request(&req(
            "bash",
            serde_json::json!({"command": "python3 hello.py || python hello.py"}),
        ));
        assert_eq!(d.tool_name, "bash");
        assert_eq!(d.detail, "python3 hello.py || python hello.py");
    }

    #[test]
    fn path_edit_shows_diff_hint() {
        let d = permission_from_request(&req(
            "str_replace",
            serde_json::json!({
                "path": "src/main.rs",
                "old_string": "foo",
                "new_string": "bar"
            }),
        ));
        assert!(d.detail.contains("src/main.rs"));
        assert!(d.detail.contains("− foo"));
        assert!(d.detail.contains("+ bar"));
    }
}
