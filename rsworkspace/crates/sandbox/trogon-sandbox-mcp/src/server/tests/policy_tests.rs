use std::sync::Arc;

use rmcp::ServerHandler;
use serde_json::json;
use trogon_sandbox_client::BusMessage;
use trogon_sandbox_contract::TenantId;

use super::support::call_with;
use crate::policy::{Policy, ToolFilter};
use crate::server::SandboxMcpServer;
use crate::server_state::ServerState;
use crate::testing::{McpSession, Reply, ScriptedBus, TestResult, test_server};

fn only(patterns: &[&str]) -> Policy {
    Policy {
        tools: ToolFilter::new(patterns.iter().map(|p| (*p).to_string()).collect(), Vec::new()),
        ..Policy::default()
    }
}

#[tokio::test]
async fn every_tool_is_listed_once_without_a_filter() -> TestResult {
    let session = McpSession::start(test_server(ScriptedBus::silent(), Policy::default())?).await?;
    let mut names = session.tool_names().await?;
    session.finish().await?;
    assert!(names.iter().all(|name| name.starts_with("sandbox_")));
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total);
    assert!(names.iter().any(|name| name == "sandbox_run_command"));
    Ok(())
}

#[tokio::test]
async fn a_filter_hides_and_blocks_the_other_tools() -> TestResult {
    let policy = only(&["sandbox_stat", "sandbox_poll_*"]);
    let session = McpSession::start(test_server(ScriptedBus::silent(), policy.clone())?).await?;
    let mut names = session.tool_names().await?;
    names.sort();
    assert_eq!(
        names,
        vec!["sandbox_poll_command".to_string(), "sandbox_stat".to_string()]
    );
    let blocked = session
        .call("sandbox_mkdir", json!({"lease_id": "l1", "path": "/d"}))
        .await?;
    assert_eq!(blocked.is_error, Some(true));
    session.finish().await?;

    let server = test_server(ScriptedBus::silent(), policy)?;
    assert!(server.get_tool("sandbox_stat").is_some());
    assert!(server.get_tool("sandbox_mkdir").is_none());
    assert!(server.get_tool("sandbox_nonexistent").is_none());
    Ok(())
}

#[tokio::test]
async fn filter_patterns_that_match_no_tool_are_reported() -> TestResult {
    let server = test_server(ScriptedBus::silent(), only(&["sandbox_stat", "typo_*"]))?;
    assert_eq!(server.unmatched_tool_patterns(), vec!["typo_*".to_string()]);
    Ok(())
}

#[tokio::test]
async fn the_server_names_its_tenant() -> TestResult {
    let server = test_server(ScriptedBus::silent(), Policy::default())?;
    let info = server.get_info();
    assert!(info.instructions.is_some_and(|text| text.contains("acme")));
    Ok(())
}

#[tokio::test]
async fn calls_without_a_key_or_a_broker_fail_cleanly() -> TestResult {
    let tenant = TenantId::new("acme")?;
    let client = Arc::new(ScriptedBus::silent().into_client());
    let keyless = ServerState::new(Policy::default(), tenant.clone(), None, Some(client));
    let unconnected = ServerState::new(Policy::default(), tenant, Some("key".into()), None);
    for state in [keyless, unconnected] {
        let session = McpSession::start(SandboxMcpServer::new(Arc::new(state))).await?;
        let result = session.call("sandbox_usage", json!({"lease_id": "l1"})).await?;
        assert_eq!(result.is_error, Some(true));
        session.finish().await?;
    }
    Ok(())
}

#[tokio::test]
async fn evidence_frames_arrive_as_logging_notifications() -> TestResult {
    let frames = vec![BusMessage {
        subject: "sandbox.evidence.l1.stdout".into(),
        payload: b"out".to_vec().into(),
    }];
    let bus = ScriptedBus::silent().with_subscribe_items(frames);
    let session = McpSession::start(test_server(bus, Policy::default())?).await?;
    let result = session
        .call("sandbox_subscribe_evidence", json!({"lease_id": "l1"}))
        .await?;
    assert_ne!(result.is_error, Some(true));
    let logged = session.recorder().logging(1).await;
    session.finish().await?;
    let first = logged.first().ok_or("no evidence arrived")?;
    assert_eq!(first["lease_id"], json!("l1"));
    assert_eq!(first["subject_suffix"], json!("stdout"));
    assert_eq!(first["payload_base64"], json!("b3V0"));
    Ok(())
}

#[tokio::test]
async fn subscribing_needs_a_connected_server() -> TestResult {
    let outcome = call_with(
        Policy::default(),
        "sandbox_subscribe_evidence",
        json!({"lease_id": "l1"}),
        vec![Reply::Timeout],
    )
    .await?;
    assert!(!outcome.is_error());
    Ok(())
}

#[tokio::test]
async fn the_evidence_pump_stops_when_the_client_is_gone() -> TestResult {
    let frames = vec![BusMessage {
        subject: "sandbox.evidence.l1.stdout".into(),
        payload: b"out".to_vec().into(),
    }];
    let (bus, gate) = ScriptedBus::silent().with_gated_subscribe(frames);
    let session = McpSession::start(test_server(bus, Policy::default())?).await?;
    let result = session
        .call("sandbox_subscribe_evidence", json!({"lease_id": "l1"}))
        .await?;
    assert_ne!(result.is_error, Some(true));
    session.finish().await?;
    gate.open();
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.dropped()).await?;
    Ok(())
}
