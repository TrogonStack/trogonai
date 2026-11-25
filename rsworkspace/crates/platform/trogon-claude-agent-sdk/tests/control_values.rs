#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    ConnectionInfo, ContextUsageResponse, FeatureSupport, InterruptReceipt, JsonObject, McpReadResourceResponse,
    McpSetServersResult, McpStatusResponse, MessageId, RewindFilesResult,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object(value: serde_json::Value) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_value(value)
}

#[test]
fn feature_support_distinguishes_unreported_from_unsupported() {
    assert_eq!(FeatureSupport::from(Some(true)), FeatureSupport::Supported);
    assert_eq!(FeatureSupport::from(Some(false)), FeatureSupport::Unsupported);
    assert_eq!(FeatureSupport::from(None), FeatureSupport::NotReported);
    assert_eq!(FeatureSupport::default(), FeatureSupport::NotReported);
}

#[test]
fn connection_info_reads_known_fields_and_keeps_the_raw_snapshot() -> TestResult {
    let raw = object(json!({
        "commands": [{"name": "compact"}],
        "models": [{"id": "model"}],
        "agents": [{"name": "reviewer"}],
        "account": null,
        "hooks_applied": true,
        "hook_on_failure_supported": false,
        "claude_code_version": "1.2.3",
        "extension": 7
    }))?;
    let info = ConnectionInfo::from_json(raw.clone().into());
    assert_eq!(info.commands.len(), 1);
    assert_eq!(info.models.len(), 1);
    assert_eq!(info.agents.len(), 1);
    assert_eq!(info.account, None);
    assert_eq!(info.hooks_applied, FeatureSupport::Supported);
    assert_eq!(info.hook_on_failure_supported, FeatureSupport::Unsupported);
    assert_eq!(info.claude_code_version.as_deref(), Some("1.2.3"));
    assert_eq!(info.additional_fields.get("extension"), Some(&json!(7)));
    assert_eq!(info.as_json(), Some(&serde_json::Value::Object(raw)));
    Ok(())
}

#[test]
fn connection_info_defaults_omitted_fields_and_reads_malformed_ones_as_absent() -> TestResult {
    let info = ConnectionInfo::from_json(json!({}));
    assert!(info.commands.is_empty());
    assert_eq!(info.hooks_applied, FeatureSupport::NotReported);
    assert_eq!(info.as_json(), Some(&json!({})));
    let malformed = json!({"commands": "not-a-list", "hooks_applied": "yes", "claude_code_version": 2});
    let info = ConnectionInfo::from_json(malformed.clone());
    assert!(info.commands.is_empty());
    assert_eq!(info.hooks_applied, FeatureSupport::NotReported);
    assert_eq!(info.claude_code_version, None);
    assert!(info.additional_fields.is_empty());
    assert_eq!(info.as_json(), Some(&malformed));
    let info = ConnectionInfo::from_json(json!(42));
    assert!(info.commands.is_empty());
    assert_eq!(info.as_json(), Some(&json!(42)));
    Ok(())
}

#[test]
fn interrupt_receipt_without_a_queue_report_is_not_reported() -> TestResult {
    for value in [json!({}), json!({"still_queued": "x"}), json!({"cancelled": []})] {
        let receipt = InterruptReceipt::from_json(object(value.clone())?);
        assert!(matches!(
            receipt,
            InterruptReceipt::NotReported { additional_fields } if additional_fields == object(value)?
        ));
    }
    Ok(())
}

#[test]
fn interrupt_receipt_drops_malformed_ids_and_ignores_a_malformed_cancelled_list() -> TestResult {
    let receipt = InterruptReceipt::from_json(object(json!({
        "still_queued": ["a", 1, null, "b"],
        "cancelled": ["c", false],
        "extension": true
    }))?);
    assert!(matches!(
        receipt,
        InterruptReceipt::Reported { still_queued, cancelled, additional_fields }
            if still_queued == [MessageId::from("a"), MessageId::from("b")]
                && cancelled == Some(vec![MessageId::from("c")])
                && additional_fields == object(json!({"extension": true}))?
    ));
    let receipt = InterruptReceipt::from_json(object(json!({"still_queued": [], "cancelled": "x"}))?);
    assert!(matches!(
        receipt,
        InterruptReceipt::Reported { still_queued, cancelled: None, .. } if still_queued.is_empty()
    ));
    Ok(())
}

#[test]
fn control_responses_keep_extension_fields() -> TestResult {
    let rewind: RewindFilesResult = serde_json::from_value(json!({
        "canRewind": true, "filesChanged": ["a"], "insertions": 1, "deletions": 2, "extra": 1
    }))?;
    assert!(rewind.can_rewind);
    assert_eq!(rewind.files_changed, Some(vec!["a".to_owned()]));
    assert_eq!(rewind.additional_fields.get("extra"), Some(&json!(1)));
    let status: McpStatusResponse =
        serde_json::from_value(json!({"mcpServers": [{"name": "s", "status": "connected"}], "extra": 1}))?;
    assert_eq!(status.mcp_servers.len(), 1);
    assert_eq!(status.mcp_servers[0].name, "s");
    assert_eq!(status.additional_fields.get("extra"), Some(&json!(1)));
    let usage: ContextUsageResponse = serde_json::from_value(json!({
        "categories": [{"name": "c", "tokens": 1, "color": "red", "isDeferred": true}],
        "totalTokens": 1, "maxTokens": 2, "rawMaxTokens": 3, "percentage": 0.5, "model": "m",
        "isAutoCompactEnabled": false, "memoryFiles": [], "mcpTools": []
    }))?;
    assert_eq!(usage.categories[0].is_deferred, Some(true));
    assert!(usage.agents.is_none() && usage.grid_rows.is_none() && usage.api_usage.is_none());
    Ok(())
}

/// Python `ContextUsageResponse` declares more keys than the minimum a reply carries; each one is typed.
#[test]
fn context_usage_types_every_python_key() -> TestResult {
    let usage: ContextUsageResponse = serde_json::from_value(json!({
        "categories": [], "totalTokens": 1, "maxTokens": 2, "rawMaxTokens": 3, "percentage": 0.5, "model": "m",
        "isAutoCompactEnabled": true, "memoryFiles": [], "mcpTools": [],
        "agents": [{"agentType": "a"}],
        "gridRows": [[{"square": 1}]],
        "autoCompactThreshold": 9,
        "deferredBuiltinTools": [{"name": "t"}],
        "systemTools": [{"name": "s"}],
        "systemPromptSections": [{"name": "p"}],
        "slashCommands": {"totalCommands": 1},
        "skills": {"totalSkills": 2},
        "messageBreakdown": {"toolCallTokens": 3},
        "apiUsage": {"input_tokens": 4}
    }))?;
    assert_eq!(usage.agents.map(|agents| agents.len()), Some(1));
    assert_eq!(usage.grid_rows.map(|rows| rows[0].len()), Some(1));
    assert_eq!(usage.auto_compact_threshold, Some(9));
    assert_eq!(usage.deferred_builtin_tools.map(|tools| tools.len()), Some(1));
    assert_eq!(usage.system_tools.map(|tools| tools.len()), Some(1));
    assert_eq!(usage.system_prompt_sections.map(|sections| sections.len()), Some(1));
    assert_eq!(
        usage
            .slash_commands
            .and_then(|commands| commands.get("totalCommands").cloned()),
        Some(json!(1))
    );
    assert_eq!(
        usage.skills.and_then(|skills| skills.get("totalSkills").cloned()),
        Some(json!(2))
    );
    assert_eq!(
        usage
            .message_breakdown
            .and_then(|parts| parts.get("toolCallTokens").cloned()),
        Some(json!(3))
    );
    assert_eq!(
        usage.api_usage.and_then(|api| api.get("input_tokens").cloned()),
        Some(json!(4))
    );
    assert!(usage.additional_fields.is_empty());
    let null_usage: ContextUsageResponse = serde_json::from_value(json!({
        "categories": [], "totalTokens": 1, "maxTokens": 2, "rawMaxTokens": 3, "percentage": 0.5, "model": "m",
        "isAutoCompactEnabled": true, "memoryFiles": [], "mcpTools": [], "apiUsage": null
    }))?;
    assert!(null_usage.api_usage.is_none());
    Ok(())
}

/// Python `McpServerStatus` and its nested types decode from the `mcpServers` list.
#[test]
fn mcp_status_types_every_python_server_key() -> TestResult {
    let status: McpStatusResponse = serde_json::from_value(json!({"mcpServers": [{
        "name": "docs", "status": "failed", "error": "refused", "scope": "project",
        "serverInfo": {"name": "docs", "version": "1"},
        "config": {"type": "http", "url": "https://example.test/mcp", "headers": {"x": "y"}},
        "tools": [{"name": "t", "description": "d", "annotations": {"readOnly": true, "destructive": false, "openWorld": true}}]
    }]}))?;
    let server = &status.mcp_servers[0];
    assert_eq!(server.error.as_deref(), Some("refused"));
    assert_eq!(server.scope.as_deref(), Some("project"));
    assert_eq!(server.server_info.as_ref().map(|info| info.name.as_str()), Some("docs"));
    assert_eq!(server.tools.as_ref().map(Vec::len), Some(1));
    assert!(server.config.is_some());
    assert_eq!(
        serde_json::to_value(server)?["config"]["url"],
        "https://example.test/mcp"
    );
    Ok(())
}

#[test]
fn mcp_set_servers_result_reads_errors() -> TestResult {
    let servers: McpSetServersResult =
        serde_json::from_value(json!({"added": ["a"], "removed": [], "errors": {"b": "failed"}}))?;
    assert_eq!(servers.errors.get("b").map(String::as_str), Some("failed"));
    Ok(())
}

/// TypeScript `readMcpResource` hands back whatever Claude Code answered, so nothing is required or dropped.
#[test]
fn mcp_read_resource_response_keeps_the_response_as_sent() -> TestResult {
    let sent = object(json!({
        "contents": [{"uri": "ui://local/app", "mimeType": "text/html", "text": "<p></p>", "_meta": {"ui": {}}}],
        "extra": 1
    }))?;
    let response = McpReadResourceResponse::from(Value::Object(sent.clone()));
    assert_eq!(response.contents().map(<[_]>::len), Some(1));
    assert_eq!(response.as_json(), &Value::Object(sent.clone()));
    assert_eq!(response.clone().into_json(), Value::Object(sent));
    let unexpected = McpReadResourceResponse::from(json!({"contents": "none"}));
    assert_eq!(unexpected.contents(), None);
    assert_eq!(unexpected.as_json()["contents"], "none");
    let text = McpReadResourceResponse::from(json!("as sent"));
    assert_eq!(text.contents(), None);
    assert_eq!(text.as_json(), &json!("as sent"));
    assert_eq!(McpReadResourceResponse::default().as_json(), &Value::Null);
    assert_eq!(McpReadResourceResponse::default().contents(), None);
    Ok(())
}
