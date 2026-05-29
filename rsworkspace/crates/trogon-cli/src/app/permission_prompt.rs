//! Tool-permission prompt rendering for the CLI UI.

use agent_client_protocol::RequestPermissionRequest;

/// One-line summary of the tool call awaiting permission: the tool title, plus
/// a truncated snippet of its raw input when present.
pub fn permission_summary(req: &RequestPermissionRequest) -> String {
    let title = req.tool_call.fields.title.as_deref().unwrap_or("tool");
    let raw = req
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_default();
    if raw.is_empty() {
        title.to_string()
    } else {
        let snippet: String = raw.chars().take(120).collect();
        format!("{title}  {snippet}")
    }
}

/// Print the permission prompt and its key options. The caller resets the
/// display and clears the current line before calling this.
pub fn print_permission_prompt(summary: &str) {
    eprintln!();
    eprintln!("┆ {summary}");
    eprintln!("[a] allow  [w] always allow  [r] reject");
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{ToolCallId, ToolCallUpdate, ToolCallUpdateFields};
    use serde_json::Value;

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
    fn summary_includes_title_and_raw_input() {
        let s = permission_summary(&req("bash", serde_json::json!({"command": "ls -la"})));
        assert!(s.starts_with("bash  "));
        assert!(s.contains("ls -la"));
    }

    #[test]
    fn summary_title_only_when_no_raw_input() {
        let mut fields = ToolCallUpdateFields::new();
        fields.title = Some("fetch_url".into());
        let r = RequestPermissionRequest::new(
            "sess-1",
            ToolCallUpdate::new(ToolCallId::new("tc-1"), fields),
            vec![],
        );
        assert_eq!(permission_summary(&r), "fetch_url");
    }
}
