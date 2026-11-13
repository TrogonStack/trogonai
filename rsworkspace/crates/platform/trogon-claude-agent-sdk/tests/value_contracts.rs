#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Public contract tests require exact valid, malformed and extension wire fixtures."
    )
)]

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use trogon_claude_agent_sdk::{
    AgentDefinition, AgentExecution, AgentMcpServerSpec, AgentName, BuiltinToolName, CallbackError, CancellationToken,
    ClaudeAgentOptions, ControlRequestId, DialogKind, EffortLevel, ElicitationResponse, HookContinuation, HookEvent,
    HookFailureMode, HookOutput, HookOutputVisibility, HookPermissionDecision, HookRegistration, HookSpecificOutput,
    InputBlock, InterruptReceipt, JsonObject, LocalMcpServer, LocalMcpServerOptions, McpServerError, McpServerName,
    McpServerOptions, McpToolInputSchema, McpToolName, McpToolOutput, McpToolRequest, McpUrl, McpUrlError, MessageId,
    MessageOrigin, OutputSchema, PermissionBehavior, PermissionMode, PermissionUpdate, PermissionUpdateDestination,
    PermissionUpdateError, Prompt, QueueCapacity, QueueCapacityError, SdkMcpServer, SdkMcpSession, SdkTool,
    SdkToolOptions, SessionId, SyncHookOutput, TaskId, Timeout, TimeoutError, ToolName, ToolResultStatus, ToolRule,
    ToolUseId, UserDialogResponse, UserInput,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object(value: Value) -> Result<JsonObject, serde_json::Error> {
    serde_json::from_value(value)
}

macro_rules! opaque {
    ($name:ident) => {{
        for value in ["", "bad\nvalue", "bad\0value", "unicode café value", "a,b c"] {
            let name = $name::from(value);
            assert_eq!(name.as_str(), value);
        }
    }};
}

#[test]
fn scalar_values_store_every_string_verbatim_and_leave_checks_to_the_runtime_boundary() {
    opaque!(MessageId);
    opaque!(ControlRequestId);
    opaque!(ToolUseId);
    opaque!(ToolName);
    opaque!(McpServerName);
    opaque!(McpToolName);
    opaque!(AgentName);
    opaque!(TaskId);
    opaque!(DialogKind);
    opaque!(SessionId);
    opaque!(Prompt);
    opaque!(ToolRule);
    opaque!(BuiltinToolName);
}

#[test]
fn prompt_rule_and_tool_conversions_preserve_text() {
    assert_eq!(Prompt::from("line one\nline two").as_str(), "line one\nline two");
    assert_eq!(ToolRule::from("Bash(git status:*)").as_str(), "Bash(git status:*)");
    assert_eq!(BuiltinToolName::from(ToolName::from("Read")).as_str(), "Read");
}

#[test]
fn timeout_and_capacity_conversions_keep_their_public_bounds() -> TestResult {
    assert!(matches!(Timeout::try_from(Duration::ZERO), Err(TimeoutError::Zero)));
    assert!(matches!(
        Timeout::try_from(Timeout::MAX + Duration::from_nanos(1)),
        Err(TimeoutError::TooLarge)
    ));
    assert_eq!(Timeout::try_from(Timeout::MAX)?.as_duration(), Timeout::MAX);
    assert!(matches!(QueueCapacity::try_from(0), Err(QueueCapacityError::Zero)));
    assert!(matches!(
        QueueCapacity::try_from(QueueCapacity::MAX + 1),
        Err(QueueCapacityError::TooLarge)
    ));
    assert_eq!(QueueCapacity::try_from(QueueCapacity::MAX)?.get(), QueueCapacity::MAX);
    Ok(())
}

#[test]
fn permission_updates_round_trip_every_operation_and_destination() -> TestResult {
    for operation in ["addRules", "replaceRules", "removeRules"] {
        for behavior in ["allow", "deny", "ask"] {
            let wire = json!({
                "type":operation,"rules":[{"toolName":"Read","ruleContent":"src/**"}],
                "behavior":behavior,"destination":"projectSettings"
            });
            let update = PermissionUpdate::from_wire_json(object(wire.clone())?)?;
            assert_eq!(update.to_wire_json()?, wire);
        }
    }
    for mode in ["default", "acceptEdits", "plan", "dontAsk", "auto", "bypassPermissions"] {
        let wire = json!({"type":"setMode","mode":mode,"destination":"cliArg"});
        assert_eq!(
            PermissionUpdate::from_wire_json(object(wire.clone())?)?.to_wire_json()?,
            wire
        );
    }
    for operation in ["addDirectories", "removeDirectories"] {
        let wire = json!({"type":operation,"directories":["project","another directory"],"destination":"session"});
        assert_eq!(
            PermissionUpdate::from_wire_json(object(wire.clone())?)?.to_wire_json()?,
            wire
        );
    }
    for (destination, literal) in [
        (PermissionUpdateDestination::UserSettings, "userSettings"),
        (PermissionUpdateDestination::ProjectSettings, "projectSettings"),
        (PermissionUpdateDestination::LocalSettings, "localSettings"),
        (PermissionUpdateDestination::Session, "session"),
        (PermissionUpdateDestination::CliArg, "cliArg"),
        (PermissionUpdateDestination::Unknown("future".into()), "future"),
    ] {
        assert_eq!(serde_json::to_value(&destination)?, literal);
        assert_eq!(
            serde_json::from_value::<PermissionUpdateDestination>(literal.into())?,
            destination
        );
    }
    for (behavior, literal) in [
        (PermissionBehavior::Allow, "allow"),
        (PermissionBehavior::Deny, "deny"),
        (PermissionBehavior::Ask, "ask"),
        (PermissionBehavior::Unknown("future".into()), "future"),
    ] {
        assert_eq!(serde_json::to_value(&behavior)?, literal);
        assert_eq!(serde_json::from_value::<PermissionBehavior>(literal.into())?, behavior);
    }
    Ok(())
}

#[test]
fn unknown_permission_values_pass_through_and_malformed_updates_fail_at_the_typed_boundary() -> TestResult {
    let future = object(json!({"type":"setMode","mode":"future","destination":"session"}))?;
    let update = PermissionUpdate::from_wire_json(future.clone())?;
    assert!(matches!(
        &update,
        PermissionUpdate::SetMode { mode: PermissionMode::Unknown(mode), .. } if mode == "future"
    ));
    assert_eq!(update.to_wire_json()?, serde_json::Value::from(future));
    for wire in [
        json!({"type":"future"}),
        json!({"type":"setMode","mode":"default","destination":"unknown"}),
        json!({"type":"addRules","rules":[],"behavior":"later","destination":"session"}),
    ] {
        let update = PermissionUpdate::from_wire_json(object(wire.clone())?)?;
        assert_eq!(update.to_wire_json()?, wire);
    }
    assert!(matches!(
        PermissionUpdate::from_wire_json(object(json!({"type":"future"}))?)?,
        PermissionUpdate::Unknown(_)
    ));
    for value in [
        json!({"destination":"session"}),
        json!({"type":"addRules","rules":{},"behavior":"allow","destination":"session"}),
        json!({"type":"removeDirectories","directories":[1],"destination":"session"}),
    ] {
        assert!(matches!(
            PermissionUpdate::from_wire_json(object(value)?),
            Err(PermissionUpdateError::Json(_))
        ));
    }
    Ok(())
}

#[test]
fn hook_event_names_preserve_all_upstream_wire_spellings() -> TestResult {
    for (event, literal) in [
        (HookEvent::PreToolUse, "PreToolUse"),
        (HookEvent::PostToolUse, "PostToolUse"),
        (HookEvent::PostToolUseFailure, "PostToolUseFailure"),
        (HookEvent::PostToolBatch, "PostToolBatch"),
        (HookEvent::Notification, "Notification"),
        (HookEvent::UserPromptSubmit, "UserPromptSubmit"),
        (HookEvent::UserPromptExpansion, "UserPromptExpansion"),
        (HookEvent::SessionStart, "SessionStart"),
        (HookEvent::SessionEnd, "SessionEnd"),
        (HookEvent::Stop, "Stop"),
        (HookEvent::StopFailure, "StopFailure"),
        (HookEvent::SubagentStart, "SubagentStart"),
        (HookEvent::SubagentStop, "SubagentStop"),
        (HookEvent::PreCompact, "PreCompact"),
        (HookEvent::PostCompact, "PostCompact"),
        (HookEvent::PreModelSwitch, "PreModelSwitch"),
        (HookEvent::PostModelSwitch, "PostModelSwitch"),
        (HookEvent::PermissionRequest, "PermissionRequest"),
        (HookEvent::PermissionDenied, "PermissionDenied"),
        (HookEvent::Setup, "Setup"),
        (HookEvent::TeammateIdle, "TeammateIdle"),
        (HookEvent::TaskCreated, "TaskCreated"),
        (HookEvent::TaskCompleted, "TaskCompleted"),
        (HookEvent::Elicitation, "Elicitation"),
        (HookEvent::ElicitationResult, "ElicitationResult"),
        (HookEvent::ConfigChange, "ConfigChange"),
        (HookEvent::WorktreeCreate, "WorktreeCreate"),
        (HookEvent::WorktreeRemove, "WorktreeRemove"),
        (HookEvent::InstructionsLoaded, "InstructionsLoaded"),
        (HookEvent::CwdChanged, "CwdChanged"),
        (HookEvent::FileChanged, "FileChanged"),
        (HookEvent::DirectoryAdded, "DirectoryAdded"),
        (HookEvent::MessageDisplay, "MessageDisplay"),
    ] {
        assert_eq!(event.as_str(), literal);
        assert_eq!(serde_json::to_value(event)?, literal);
        assert_eq!(serde_json::from_value::<HookEvent>(literal.into())?, event);
    }
    Ok(())
}

#[test]
fn hook_outputs_preserve_decisions_context_and_reserved_event_identity() -> TestResult {
    assert_eq!(HookOutput::default().to_wire_json()?, json!({}));
    assert_eq!(
        HookOutput::block("policy").to_wire_json()?,
        json!({"decision":"block","reason":"policy"})
    );
    assert_eq!(
        HookOutput::Async { timeout: None }.to_wire_json()?,
        json!({"async":true})
    );
    for (decision, literal) in [
        (HookPermissionDecision::Allow, "allow"),
        (HookPermissionDecision::Deny, "deny"),
        (HookPermissionDecision::Ask, "ask"),
        (HookPermissionDecision::Defer, "defer"),
    ] {
        let output = HookOutput::Sync(Box::new(SyncHookOutput {
            continuation: Some(HookContinuation::Continue),
            visibility: Some(HookOutputVisibility::Suppressed),
            stop_reason: Some("stopped".into()),
            block_reason: Some("fallback reason".into()),
            reason: Some("explicit reason".into()),
            system_message: Some("host notice".into()),
            specific: Some(HookSpecificOutput::PreToolUse {
                permission_decision: Some(decision),
                permission_decision_reason: Some("decision detail".into()),
                updated_input: Some(object(json!({"path":"README.md"}))?),
                additional_context: Some("context".into()),
            }),
        }))
        .to_wire_json()?;
        assert_eq!(output["continue"], true);
        assert_eq!(output["suppressOutput"], true);
        assert_eq!(output["reason"], "explicit reason");
        assert_eq!(output["stopReason"], "stopped");
        assert_eq!(output["systemMessage"], "host notice");
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], literal);
        assert_eq!(
            output["hookSpecificOutput"]["permissionDecisionReason"],
            "decision detail"
        );
        assert_eq!(
            output["hookSpecificOutput"]["updatedInput"],
            json!({"path":"README.md"})
        );
        assert_eq!(output["hookSpecificOutput"]["additionalContext"], "context");
    }
    let context = HookOutput::Sync(Box::new(SyncHookOutput {
        specific: Some(HookSpecificOutput::AdditionalContext {
            event: HookEvent::SessionStart,
            context: "start context".into(),
        }),
        ..SyncHookOutput::default()
    }))
    .to_wire_json()?;
    assert_eq!(
        context,
        json!({"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"start context"}})
    );
    let extension = HookOutput::Sync(Box::new(SyncHookOutput {
        specific: Some(HookSpecificOutput::Event {
            event: HookEvent::PostToolUseFailure,
            fields: object(json!({"hookEventName":"spoof","nested":{"detail":1}}))?,
        }),
        ..SyncHookOutput::default()
    }))
    .to_wire_json()?;
    assert_eq!(
        extension,
        json!({"hookSpecificOutput":{"hookEventName":"PostToolUseFailure","nested":{"detail":1}}})
    );
    Ok(())
}

#[test]
fn callback_configuration_debug_reports_structure_without_callback_or_input_details() -> TestResult {
    let registration = HookRegistration {
        event: HookEvent::SessionStart,
        matcher: Some("fixture-private-matcher".into()),
        callbacks: Vec::new(),
        timeout: None,
        on_failure: Some(HookFailureMode::Continue),
    };
    assert_eq!(
        registration.to_wire_json(&["callback-one".into()])?,
        json!({
            "matcher":"fixture-private-matcher","hookCallbackIds":["callback-one"],"onFailure":"continue"
        })
    );
    let unmatched = HookRegistration {
        matcher: None,
        ..registration.clone()
    };
    assert_eq!(
        unmatched.to_wire_json(&["callback-one".into()])?,
        json!({"hookCallbackIds":["callback-one"],"onFailure":"continue"})
    );
    let debug = format!("{registration:?}");
    assert!(debug.contains("callback_count: 0"));
    assert!(!debug.contains("fixture-private-matcher"));
    let options = ClaudeAgentOptions {
        model: Some("fixture-private-model".into()),
        env: trogon_claude_agent_sdk::Environment::from([("FIXTURE_CONFIG".into(), "fixture-private-value".into())]),
        ..ClaudeAgentOptions::default()
    };
    let debug = format!("{options:?}");
    assert!(debug.contains("limits"));
    assert!(!debug.contains("fixture-private-model"));
    assert!(!debug.contains("fixture-private-value"));
    Ok(())
}

#[test]
fn mcp_transport_options_preserve_wire_values_but_redact_sensitive_debug_values() -> TestResult {
    let transports = [
        (
            McpServerOptions::Stdio {
                command: PathBuf::from("fixture-private-command"),
                args: vec!["fixture-private-argument".into()],
                env: trogon_claude_agent_sdk::Environment::from([(
                    "FIXTURE_CONFIG".into(),
                    "fixture-private-value".into(),
                )]),
            },
            json!({"type":"stdio","command":"fixture-private-command","args":["fixture-private-argument"],"env":{"FIXTURE_CONFIG":"fixture-private-value"}}),
        ),
        (
            McpServerOptions::Http {
                url: McpUrl::new("https://example.com/mcp?fixture=private")?,
                headers: BTreeMap::from([("X-Fixture".into(), "fixture-private-header".into())]),
            },
            json!({"type":"http","url":"https://example.com/mcp?fixture=private","headers":{"X-Fixture":"fixture-private-header"}}),
        ),
        (
            McpServerOptions::Sse {
                url: McpUrl::new("http://example.com/events")?,
                headers: BTreeMap::new(),
            },
            json!({"type":"sse","url":"http://example.com/events","headers":{}}),
        ),
    ];
    for (transport, expected) in transports {
        assert_eq!(transport.to_wire_json()?, expected);
        let debug = format!("{transport:?}");
        assert!(!debug.contains("fixture-private-argument"));
        assert!(!debug.contains("fixture-private-value"));
        assert!(!debug.contains("fixture-private-header"));
        assert!(!debug.contains("example.com"));
    }
    let plain = McpServerOptions::Stdio {
        command: PathBuf::from("fixture-command"),
        args: vec![trogon_claude_agent_sdk::ConfigValue::Plain("--fixture-flag".into())],
        env: trogon_claude_agent_sdk::Environment::from([("FIXTURE_CONFIG".into(), "fixture-private-value".into())]),
    };
    let debug = format!("{plain:?}");
    for shown in ["fixture-command", "--fixture-flag", "FIXTURE_CONFIG", "<redacted>"] {
        assert!(debug.contains(shown), "{debug}");
    }
    assert!(matches!(McpUrl::new("not a URL"), Err(McpUrlError::Parse(_))));
    assert!(matches!(McpUrl::new("file:///fixture"), Err(McpUrlError::Scheme)));
    assert!(matches!(
        McpUrl::new("https://user:invalid@localhost/mcp"),
        Err(McpUrlError::Credentials)
    ));
    assert!(matches!(
        McpUrl::new("https://example.com/mcp#fragment"),
        Err(McpUrlError::Fragment)
    ));
    assert_eq!(
        format!("{:?}", McpUrl::new("https://example.com/mcp")?),
        "McpUrl { .. }"
    );
    Ok(())
}

#[test]
fn agent_definitions_preserve_inline_and_named_mcp_scopes() -> TestResult {
    for (effort, literal) in [
        (EffortLevel::Low, "low"),
        (EffortLevel::Medium, "medium"),
        (EffortLevel::High, "high"),
        (EffortLevel::XHigh, "xhigh"),
        (EffortLevel::Max, "max"),
    ] {
        let mut agent = AgentDefinition::new("review", Prompt::from("review the change"));
        agent.effort = Some(effort);
        agent.execution = Some(AgentExecution::Foreground);
        agent.permission_mode = Some(PermissionMode::DontAsk);
        agent.max_turns = NonZeroUsize::new(3);
        agent.mcp_servers = Some(vec![
            AgentMcpServerSpec::Named(McpServerName::from("named")),
            AgentMcpServerSpec::Inline(BTreeMap::from([(
                McpServerName::from("inline"),
                McpServerOptions::Sse {
                    url: McpUrl::new("https://example.com/events")?,
                    headers: BTreeMap::new(),
                },
            )])),
        ]);
        let wire = agent.to_wire_json()?;
        assert_eq!(wire["effort"], literal);
        assert_eq!(wire["background"], false);
        assert_eq!(
            wire["mcpServers"],
            json!(["named", {"inline":{"type":"sse","url":"https://example.com/events","headers":{}}}])
        );
    }
    Ok(())
}

/// Both upstream SDKs read an interrupt queue report leniently: a non-array `still_queued` means no report, and
/// non-string ids or a non-array `cancelled` are dropped.
#[test]
fn s16_interrupt_receipts_never_fail_on_a_malformed_queue_report() -> TestResult {
    for value in [json!({}), json!({"still_queued":7}), json!({"still_queued":"one"})] {
        assert!(matches!(
            InterruptReceipt::from_json(object(value)?),
            InterruptReceipt::NotReported { .. }
        ));
    }
    let receipt = InterruptReceipt::from_json(object(
        json!({"still_queued":["one",2,null,"two"],"cancelled":"three"}),
    )?);
    let InterruptReceipt::Reported {
        still_queued,
        cancelled,
        ..
    } = receipt
    else {
        return Err("missing queue report".into());
    };
    assert_eq!(still_queued, vec![MessageId::from("one"), MessageId::from("two")]);
    assert_eq!(cancelled, None);
    Ok(())
}

/// Both upstream SDKs forward user content blocks verbatim; the CLI and the Messages API validate them.
#[test]
fn s18_structured_input_blocks_pass_through_unvalidated() -> TestResult {
    for value in [
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"TQ=="}}),
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"===="}}),
        json!({"type":"image","source":{"type":"base64","media_type":"application/pdf","data":"TQ=="}}),
        json!({"type":"image","source":{"type":"url","url":"https://user:secret@localhost/image"}}),
        json!({"type":"image","source":{"type":"url","url":"not a URL"}}),
        json!({"type":"document","source":{"type":"text","media_type":"text/plain","data":""}}),
        json!({"type":"tool_result","tool_use_id":"tool-one","content":[4]}),
        json!({"type":"tool_result","tool_use_id":""}),
        json!({"type":"future","payload":1}),
        json!({"type":""}),
        json!({}),
    ] {
        let object = object(value.clone())?;
        assert_eq!(InputBlock::from_json(object.clone()).as_json(), &object);
        assert_eq!(InputBlock::from(object.clone()).as_json(), &object);
        let wire = UserInput::blocks(vec![InputBlock::from_json(object)]).to_wire_json()?;
        assert_eq!(wire["message"]["content"], json!([value]));
    }
    assert_eq!(
        UserInput::blocks(Vec::new()).to_wire_json()?["message"]["content"],
        json!([])
    );
    Ok(())
}

#[test]
fn input_identity_and_origin_round_trip_without_losing_extension_fields() -> TestResult {
    let origin = MessageOrigin::new("host", object(json!({"trace":"one"}))?)?;
    assert_eq!(origin.kind(), "host");
    assert_eq!(origin.additional_fields(), &object(json!({"trace":"one"}))?);
    let encoded = serde_json::to_value(origin.clone())?;
    let decoded: MessageOrigin = serde_json::from_value(encoded.clone())?;
    assert_eq!(decoded.kind(), "host");
    assert_eq!(serde_json::to_value(decoded)?, encoded);
    assert_eq!(serde_json::from_value::<MessageOrigin>(json!({"kind":""}))?.kind(), "");
    let mut input = UserInput::blocks(vec![InputBlock::text("hello")]);
    input.uuid = Some(MessageId::from("message-one"));
    input.session_id = Some(SessionId::from("session-one"));
    input.parent_tool_use_id = Some(ToolUseId::from("parent-one"));
    input.origin = Some(origin);
    assert_eq!(
        input.to_wire_json()?,
        json!({
            "type":"user","session_id":"session-one","parent_tool_use_id":"parent-one",
            "message":{"role":"user","content":[{"type":"text","text":"hello"}]},
            "uuid":"message-one","origin":{"kind":"host","trace":"one"}
        })
    );
    Ok(())
}

#[test]
fn dialog_responses_preserve_cancel_decline_and_optional_content() -> TestResult {
    for (response, expected) in [
        (
            ElicitationResponse::Accept { content: None },
            json!({"action":"accept"}),
        ),
        (
            ElicitationResponse::Accept {
                content: Some(object(json!({"approved":true}))?),
            },
            json!({"action":"accept","content":{"approved":true}}),
        ),
        (ElicitationResponse::Decline, json!({"action":"decline"})),
        (ElicitationResponse::Cancel, json!({"action":"cancel"})),
    ] {
        assert_eq!(response.to_wire_json()?, expected);
    }
    assert_eq!(
        UserDialogResponse::Cancelled.to_wire_json()?,
        json!({"behavior":"cancelled"})
    );
    assert_eq!(
        UserDialogResponse::Completed {
            result: json!({"value":1})
        }
        .to_wire_json()?,
        json!({"behavior":"completed","result":{"value":1}})
    );
    Ok(())
}

fn registry() -> Result<(LocalMcpServer, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let options = SdkToolOptions {
        name: McpToolName::from("inspect"),
        description: "fixture-private-description".into(),
        input_schema: McpToolInputSchema::new(object(
            json!({"type":"object","required":["status"],"properties":{"status":{"type":"string"}}}),
        )?)?,
        handler: Arc::new(move |request: McpToolRequest| {
            let calls = Arc::clone(&observed);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                if request.arguments["status"] == "failure" {
                    return Err(CallbackError::Failed(Box::new(std::io::Error::from(
                        std::io::ErrorKind::PermissionDenied,
                    ))));
                }
                let mut output = McpToolOutput::text("reply");
                output.structured_content =
                    Some(object(json!({"accepted":true})).map_err(|error| CallbackError::Failed(Box::new(error)))?);
                output.status = match request.arguments["status"].as_str() {
                    Some("unreported") => ToolResultStatus::NotReported,
                    Some("error") => ToolResultStatus::Error,
                    _ => ToolResultStatus::Success,
                };
                Ok(output)
            }
        }),
    };
    assert!(!format!("{options:?}").contains("fixture-private-description"));
    let tool = SdkTool::from(options);
    assert!(!format!("{tool:?}").contains("fixture-private-description"));
    let server_options = LocalMcpServerOptions {
        name: McpServerName::from("registry"),
        version: "fixture-private-version".into(),
        tools: tool.into(),
    };
    assert!(!format!("{server_options:?}").contains("fixture-private-version"));
    let server = LocalMcpServer::from(server_options);
    assert!(!format!("{server:?}").contains("fixture-private-version"));
    Ok((server, calls))
}

async fn rpc(session: &dyn SdkMcpSession, value: Value) -> Result<JsonObject, Box<dyn std::error::Error>> {
    session
        .request(object(value)?, CancellationToken::new())
        .await?
        .ok_or_else(|| std::io::Error::other("missing RPC response").into())
}

fn initialize(version: &str) -> Value {
    json!({"jsonrpc":"2.0","id":"initialize","method":"initialize","params":{
        "protocolVersion":version,"capabilities":{},"clientInfo":{"name":"contracts","version":"1"}
    }})
}

async fn ready(server: &LocalMcpServer) -> Result<Arc<dyn SdkMcpSession>, Box<dyn std::error::Error>> {
    let session = server.open_session();
    rpc(session.as_ref(), initialize("2025-11-25")).await?;
    assert!(
        session
            .request(
                object(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?,
                CancellationToken::new()
            )
            .await?
            .is_none()
    );
    Ok(session)
}

#[tokio::test]
async fn s07_s09_s10_local_mcp_initialize_negotiates_like_upstream_and_accepts_reinitialize() -> TestResult {
    let (server, calls) = registry()?;
    let uninitialized = server.open_session();
    assert!(matches!(
        uninitialized.request(JsonObject::new(), CancellationToken::new()).await,
        Err(McpServerError::Json(_))
    ));
    for request in [
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
    ] {
        let response = rpc(uninitialized.as_ref(), request).await?;
        assert_eq!(response["error"]["code"], -32600);
    }
    assert_eq!(
        rpc(
            uninitialized.as_ref(),
            json!({"jsonrpc":"2.0","id":u64::MAX,"method":"ping"})
        )
        .await?["result"],
        json!({})
    );
    assert!(
        uninitialized
            .request(
                object(json!({"jsonrpc":"2.0","method":"unrelated/notification"}))?,
                CancellationToken::new()
            )
            .await?
            .is_none()
    );
    let invalid_notification = rpc(uninitialized.as_ref(), json!({"jsonrpc":"1.0","method":"ping"})).await?;
    assert_eq!(invalid_notification["id"], Value::Null);
    assert_eq!(invalid_notification["error"]["code"], -32600);
    for (requested, negotiated) in [
        ("2024-10-07", "2024-10-07"),
        ("2024-11-05", "2024-11-05"),
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("future-version", "2025-11-25"),
        ("", "2025-11-25"),
    ] {
        let session = server.open_session();
        for _ in 0..2 {
            assert_eq!(
                rpc(session.as_ref(), initialize(requested)).await?["result"]["protocolVersion"],
                negotiated
            );
        }
    }
    let session = server.open_session();
    let mut request = initialize("2025-11-25");
    request["params"]["clientInfo"] = json!({"name":"contracts"});
    assert_eq!(rpc(session.as_ref(), request).await?["error"]["code"], -32602);
    let signal = CancellationToken::new();
    signal.cancel();
    assert!(matches!(
        uninitialized
            .request(object(json!({"jsonrpc":"2.0","id":1,"method":"ping"}))?, signal)
            .await,
        Err(McpServerError::Callback(CallbackError::Cancelled))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

/// The TypeScript SDK warns about tool names outside its MCP naming pattern but still registers and lists them.
#[tokio::test]
async fn s11_local_mcp_lists_tools_whose_names_upstream_only_warns_about() -> TestResult {
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("needs review!"),
        description: "listed".into(),
        input_schema: McpToolInputSchema::new(object(json!({"type":"object"}))?)?,
        handler: Arc::new(|_request: McpToolRequest| async { Ok::<_, CallbackError>(McpToolOutput::text("ok")) }),
    });
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("names"),
        version: "1".into(),
        tools: tool.into(),
    });
    let session = ready(&server).await?;
    let listed = rpc(
        session.as_ref(),
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}),
    )
    .await?;
    assert_eq!(listed["result"]["tools"][0]["name"], "needs review!");
    Ok(())
}

#[tokio::test]
async fn m02_m03_local_mcp_reports_tool_failures_as_error_results_before_invoking_tools() -> TestResult {
    let (server, calls) = registry()?;
    let session = ready(&server).await?;
    for (request, code) in [
        (
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}),
            -32602,
        ),
        (json!({"jsonrpc":"2.0","id":4,"method":"unknown/method"}), -32601),
    ] {
        assert_eq!(rpc(session.as_ref(), request).await?["error"]["code"], code);
    }
    let unknown = rpc(
        session.as_ref(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"unknown"}}),
    )
    .await?;
    assert_eq!(unknown["result"]["isError"], true);
    assert_eq!(unknown["result"]["content"][0]["text"], "Tool unknown not found");
    let invalid = rpc(
        session.as_ref(),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"inspect","arguments":{"status":4}}}),
    )
    .await?;
    assert_eq!(invalid["result"]["isError"], true);
    assert!(
        invalid["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("Input validation error: Invalid arguments for tool inspect: "))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for (status, expected) in [("success", Some(false)), ("error", Some(true)), ("unreported", None)] {
        let response = rpc(session.as_ref(), json!({"jsonrpc":"2.0","id":status,"method":"tools/call","params":{"name":"inspect","arguments":{"status":status}}})).await?;
        assert_eq!(response["result"]["content"][0]["text"], "reply");
        assert_eq!(response["result"]["structuredContent"], json!({"accepted":true}));
        assert_eq!(response["result"].get("isError").and_then(Value::as_bool), expected);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let failure = rpc(session.as_ref(), json!({"jsonrpc":"2.0","id":"failure","method":"tools/call","params":{"name":"inspect","arguments":{"status":"failure"}}})).await?;
    assert_eq!(failure["result"]["isError"], true);
    assert!(failure["result"]["content"][0]["text"].is_string());
    let signal = CancellationToken::new();
    signal.cancel();
    assert!(matches!(
        session
            .request(
                object(json!({"jsonrpc":"2.0","id":"cancelled","method":"tools/call","params":{"name":"inspect","arguments":{"status":"success"}}}))?,
                signal
            )
            .await,
        Err(McpServerError::Callback(CallbackError::Cancelled))
    ));
    Ok(())
}

#[test]
fn output_schema_debug_does_not_disclose_schema_contents() -> TestResult {
    let schema = OutputSchema::new(object(json!({"type":"object","description":"fixture-private-schema"}))?)?;
    assert!(schema.is_valid(&json!({})));
    assert_eq!(format!("{schema:?}"), "OutputSchema { .. }");
    Ok(())
}

#[tokio::test]
async fn s08_local_mcp_discovery_does_not_wait_for_the_initialized_notice_and_supports_empty_registries() -> TestResult
{
    let empty = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("empty"),
        version: "1".into(),
        tools: Default::default(),
    });
    let (populated, calls) = registry()?;
    for (server, expected) in [
        (&empty, json!([])),
        (
            &populated,
            json!([{
                "name":"inspect", "description":"fixture-private-description",
                "inputSchema":{"type":"object","required":["status"],"properties":{"status":{"type":"string"}}}
            }]),
        ),
    ] {
        let session = server.open_session();
        let early = rpc(
            session.as_ref(),
            json!({"jsonrpc":"2.0","id":"early","method":"tools/list"}),
        )
        .await?;
        assert_eq!(early["id"], "early");
        assert_eq!(early["result"]["tools"], expected);
        rpc(session.as_ref(), initialize("2025-11-25")).await?;
        let waiting = rpc(
            session.as_ref(),
            json!({"jsonrpc":"2.0","id":"waiting","method":"tools/list"}),
        )
        .await?;
        assert_eq!(waiting["id"], "waiting");
        assert_eq!(waiting["result"]["tools"], expected);
        assert!(
            session
                .request(
                    object(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?,
                    CancellationToken::new()
                )
                .await?
                .is_none()
        );
        let listed = rpc(
            session.as_ref(),
            json!({"jsonrpc":"2.0","id":"listed","method":"tools/list"}),
        )
        .await?;
        assert_eq!(listed["id"], "listed");
        assert_eq!(listed["result"]["tools"], expected);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}
