#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Protocol error tests construct invalid boundary receipts."
    )
)]

use serde_json::{Value, json};
use std::{error::Error as StdError, io};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, ControlOperation, Error, JsonObject, PermissionUpdate, ProtocolError,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::ScriptedProcessSpawner;

type TestResult = Result<(), Box<dyn StdError>>;

#[test]
fn control_timeout_diagnostics_identify_each_wire_operation() {
    for (operation, name) in [
        (ControlOperation::Initialize, "initialize"),
        (ControlOperation::Interrupt, "interrupt"),
        (ControlOperation::SetModel, "set_model"),
        (ControlOperation::SetPermissionMode, "set_permission_mode"),
        (ControlOperation::RewindFiles, "rewind_files"),
        (ControlOperation::McpStatus, "mcp_status"),
        (ControlOperation::ContextUsage, "get_context_usage"),
        (ControlOperation::McpReconnect, "mcp_reconnect"),
        (ControlOperation::McpToggle, "mcp_toggle"),
        (ControlOperation::McpReadResource, "mcp_read_resource"),
        (ControlOperation::StopTask, "stop_task"),
        (ControlOperation::BackgroundTasks, "background_tasks"),
        (ControlOperation::McpSetServers, "mcp_set_servers"),
    ] {
        let error = Error::ControlTimeout { operation };
        assert_eq!(error.to_string(), format!("Control request timeout: {name}"));
        assert!(error.source().is_none());
    }
}

#[test]
fn protocol_failures_retain_typed_sources_and_safe_context() -> TestResult {
    let failures = [
        (
            ProtocolError::from(serde_json::from_str::<Value>("{").err().ok_or("JSON accepted")?),
            "invalid JSON protocol frame",
        ),
        (
            ProtocolError::from(
                PermissionUpdate::from_wire_json(JsonObject::new())
                    .err()
                    .ok_or("permission accepted")?,
            ),
            "invalid permission update",
        ),
    ];
    for (error, context) in failures {
        assert_eq!(error.to_string(), context);
        let source = error.source().ok_or("missing typed protocol source")?;
        let typed = match &error {
            ProtocolError::Json(_) => source.is::<serde_json::Error>(),
            ProtocolError::PermissionUpdate(_) => source.is::<trogon_claude_agent_sdk::PermissionUpdateError>(),
            ProtocolError::DuplicateControlRequest | ProtocolError::InvalidMessage(_) => false,
        };
        assert!(typed);
        let outer = Error::from(error);
        assert_eq!(outer.to_string(), context);
    }
    let error = ProtocolError::DuplicateControlRequest;
    assert_eq!(error.to_string(), "duplicate incoming control request ID");
    assert!(error.source().is_none());
    let error = ProtocolError::InvalidMessage("Message missing 'type' field".to_owned());
    assert_eq!(error.to_string(), "Message missing 'type' field");
    assert!(error.source().is_none());
    Ok(())
}

#[tokio::test]
async fn rejected_control_retains_host_details_without_formatting_them() -> TestResult {
    const DETAIL: &str = "rejection-detail-canary";
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(harness.stdin).lines();
        let mut stdout = harness.stdout;
        let request: Value = serde_json::from_str(
            &lines
                .next_line()
                .await?
                .ok_or_else(|| io::Error::other("missing initialize"))?,
        )?;
        stdout.write_all(format!("{}\n", json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{}}})).as_bytes()).await?;
        let request: Value = serde_json::from_str(
            &lines
                .next_line()
                .await?
                .ok_or_else(|| io::Error::other("missing control"))?,
        )?;
        assert_eq!(request["request"]["subtype"], "set_model");
        stdout.write_all(format!("{}\n", json!({"type":"control_response","response":{"subtype":"error","request_id":request["request_id"],"error":DETAIL}})).as_bytes()).await?;
        assert!(lines.next_line().await?.is_none());
        drop(stdout);
        drop(harness.stderr);
        harness.observer.complete(0);
        Ok::<_, io::Error>(())
    });
    let client = ClaudeSdkClient::connect_with_adapters(
        ClaudeAgentOptions {
            cli_path: Some("scripted-claude".into()),
            ..ClaudeAgentOptions::default()
        },
        RuntimeAdapters {
            environment: FixedEnvironment {
                directory: "unused-memory-directory".into(),
                variables: Default::default(),
            },
            process,
            filesystem: NativeFileSystem,
        },
    )
    .await?;
    let error = client
        .handle()
        .set_model(None)
        .await
        .err()
        .ok_or("control was accepted")?;
    let Error::ControlRejected {
        operation,
        ref rejection,
    } = error
    else {
        return Err("wrong control failure".into());
    };
    assert_eq!(operation, ControlOperation::SetModel);
    assert_eq!(rejection.message(), DETAIL);
    assert_eq!(error.to_string(), "the set_model control request was rejected");
    assert!(!format!("{error:?}").contains(DETAIL));
    assert_eq!(
        error.source().ok_or("missing rejection")?.to_string(),
        "Claude Code rejected the control request"
    );
    client.disconnect().await?;
    server.await??;
    Ok(())
}
