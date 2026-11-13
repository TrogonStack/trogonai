#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use serde_json::json;
use trogon_claude_agent_sdk::{
    ConnectionInfo, ContextUsageResponse, FeatureSupport, InterruptReceipt, JsonObject, McpSetServersResult,
    McpStatusResponse, MessageId, RewindFilesResult,
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
    assert_eq!(info.as_json(), &serde_json::Value::Object(raw));
    Ok(())
}

#[test]
fn connection_info_defaults_omitted_fields_and_reads_malformed_ones_as_absent() -> TestResult {
    let info = ConnectionInfo::from_json(json!({}));
    assert!(info.commands.is_empty());
    assert_eq!(info.hooks_applied, FeatureSupport::NotReported);
    assert_eq!(info.as_json(), &json!({}));
    let malformed = json!({"commands": "not-a-list", "hooks_applied": "yes", "claude_code_version": 2});
    let info = ConnectionInfo::from_json(malformed.clone());
    assert!(info.commands.is_empty());
    assert_eq!(info.hooks_applied, FeatureSupport::NotReported);
    assert_eq!(info.claude_code_version, None);
    assert!(info.additional_fields.is_empty());
    assert_eq!(info.as_json(), &malformed);
    let info = ConnectionInfo::from_json(json!(42));
    assert!(info.commands.is_empty());
    assert_eq!(info.as_json(), &json!(42));
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
    let status: McpStatusResponse = serde_json::from_value(json!({"mcpServers": [{"name": "s"}], "extra": 1}))?;
    assert_eq!(status.mcp_servers.len(), 1);
    let usage: ContextUsageResponse = serde_json::from_value(json!({
        "categories": [{"name": "c", "tokens": 1, "color": "red", "isDeferred": true}],
        "totalTokens": 1, "maxTokens": 2, "rawMaxTokens": 3, "percentage": 0.5, "model": "m",
        "isAutoCompactEnabled": false, "memoryFiles": [], "mcpTools": []
    }))?;
    assert_eq!(usage.categories[0].is_deferred, Some(true));
    let servers: McpSetServersResult =
        serde_json::from_value(json!({"added": ["a"], "removed": [], "errors": {"b": "failed"}}))?;
    assert_eq!(servers.errors.get("b").map(String::as_str), Some("failed"));
    Ok(())
}
