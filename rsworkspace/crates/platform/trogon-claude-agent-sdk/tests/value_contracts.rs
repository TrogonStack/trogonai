#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Public contract tests require exact valid, malformed and extension wire fixtures."
    )
)]

use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
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
    InputBlock, InterruptReceipt, JsonObject, LocalMcpServer, LocalMcpServerOptions, McpConfig, McpRemoteServerOptions,
    McpSdkServerOptions, McpServerDialect, McpServerError, McpServerName, McpServerOptions, McpServerToolPolicy,
    McpStdioServerOptions, McpToolInputSchema, McpToolName, McpToolOrgMaxPermission, McpToolOutput,
    McpToolPermissionPolicy, McpToolRequest, McpUrl, McpUrlError, MessageId, MessageOrigin, OutputSchema,
    PermissionBehavior, PermissionMode, PermissionUpdate, PermissionUpdateDestination, PermissionUpdateError, Prompt,
    QueueCapacity, QueueCapacityError, SdkMcpServer, SdkMcpSession, SdkTool, SdkToolOptions, SessionId, SyncHookOutput,
    TaskId, Timeout, TimeoutError, ToolName, ToolResultStatus, ToolRule, ToolUseId, UserDialogResponse, UserInput,
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
    assert!(matches!(Timeout::checked(Duration::ZERO), Err(TimeoutError::Zero)));
    assert!(matches!(
        Timeout::checked(Timeout::MAX + Duration::from_nanos(1)),
        Err(TimeoutError::TooLarge)
    ));
    assert_eq!(Timeout::checked(Timeout::MAX)?.as_duration(), Timeout::MAX);
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
    let mut headers = McpRemoteServerOptions::new("https://example.com/mcp?fixture=private");
    headers.headers = Some(BTreeMap::from([("X-Fixture".into(), "fixture-private-header".into())]));
    let transports = [
        (
            McpServerOptions::Stdio(McpStdioServerOptions {
                command: PathBuf::from("fixture-private-command"),
                declare_type: true,
                args: Some(vec!["fixture-private-argument".into()]),
                env: Some(trogon_claude_agent_sdk::Environment::from([(
                    "FIXTURE_CONFIG".into(),
                    "fixture-private-value".into(),
                )])),
                ..McpStdioServerOptions::default()
            }),
            json!({"type":"stdio","command":"fixture-private-command","args":["fixture-private-argument"],"env":{"FIXTURE_CONFIG":"fixture-private-value"}}),
        ),
        (
            McpServerOptions::Http(headers),
            json!({"type":"http","url":"https://example.com/mcp?fixture=private","headers":{"X-Fixture":"fixture-private-header"}}),
        ),
        (
            McpServerOptions::Sse(McpRemoteServerOptions::new("http://example.com/events")),
            json!({"type":"sse","url":"http://example.com/events"}),
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
    let plain = McpServerOptions::Stdio(McpStdioServerOptions {
        command: PathBuf::from("fixture-command"),
        args: Some(vec![trogon_claude_agent_sdk::ConfigValue::Plain(
            "--fixture-flag".into(),
        )]),
        env: Some(trogon_claude_agent_sdk::Environment::from([(
            "FIXTURE_CONFIG".into(),
            "fixture-private-value".into(),
        )])),
        ..McpStdioServerOptions::default()
    });
    let debug = format!("{plain:?}");
    for shown in ["fixture-command", "--fixture-flag", "FIXTURE_CONFIG", "<redacted>"] {
        assert!(debug.contains(shown), "{debug}");
    }
    assert_eq!(
        format!("{:?}", McpUrl::from("https://example.com/mcp")),
        "McpUrl { .. }"
    );
    Ok(())
}

#[test]
fn mcp_extra_fields_and_raw_config_stay_out_of_debug_output() -> TestResult {
    let extra = JsonObject::from_iter([("oauth".into(), json!({"clientSecret": "fixture-private-extra"}))]);
    let mut remote = McpRemoteServerOptions::new("https://example.com/mcp");
    remote.extra = extra.clone();
    let servers = [
        McpServerOptions::Stdio(McpStdioServerOptions {
            command: PathBuf::from("server"),
            extra: extra.clone(),
            ..McpStdioServerOptions::default()
        }),
        McpServerOptions::Http(remote),
        McpServerOptions::Sdk(McpSdkServerOptions {
            name: "fixture".into(),
            extra,
            ..McpSdkServerOptions::default()
        }),
    ];
    for server in servers {
        assert_eq!(server.to_wire_json()?["oauth"]["clientSecret"], "fixture-private-extra");
        let debug = format!("{server:?}");
        assert!(!debug.contains("fixture-private-extra"), "{debug}");
        assert!(debug.contains("extra: <redacted>"), "{debug}");
    }
    let raw = r#"{"mcpServers":{"remote":{"headers":{"Authorization":"Bearer fixture-private-token"}}}}"#;
    let configs = [
        McpConfig::from(raw),
        McpConfig::from(raw.to_owned()),
        McpConfig::from(std::ffi::OsString::from(raw)),
        McpConfig::from(PathBuf::from(raw)),
    ];
    for config in &configs {
        assert_eq!(config.as_os_str(), raw);
        assert_eq!(config, &configs[0]);
    }
    let options = ClaudeAgentOptions {
        mcp_config: Some(configs[0].clone()),
        ..ClaudeAgentOptions::default()
    };
    let debug = format!("{options:?}");
    assert!(!debug.contains("fixture-private-token"));
    assert!(debug.contains("mcp_config: Some(<redacted>)"), "{debug}");
    Ok(())
}

/// Both SDKs take a stdio command as a string, so a path that is not UTF-8 fails instead of becoming a
/// different command.
#[cfg(unix)]
#[test]
fn mcp_stdio_command_that_is_not_utf8_is_rejected() {
    let server = McpServerOptions::Stdio(McpStdioServerOptions {
        command: PathBuf::from(std::ffi::OsStr::from_bytes(b"/srv/fixture-\xff")),
        ..McpStdioServerOptions::default()
    });
    let error = server.to_wire_json().err().map(|error| error.to_string());
    assert_eq!(error.as_deref(), Some("MCP server command is not valid UTF-8"));
}

/// Neither upstream SDK checks MCP URLs, so `McpUrl::from` keeps any text and the checked constructor
/// is opt-in.
#[test]
fn r19_mcp_urls_are_verbatim_unless_validation_is_requested() -> TestResult {
    for raw in [
        "not a URL",
        "file:///fixture",
        "https://user:pass@localhost/mcp",
        "HTTPS://Example.com:443/a/../b#f",
    ] {
        let server = McpServerOptions::Http(McpRemoteServerOptions::new(raw));
        assert_eq!(server.to_wire_json()?["url"], raw);
    }
    assert!(matches!(McpUrl::validated("not a URL"), Err(McpUrlError::Parse(_))));
    assert!(matches!(McpUrl::validated("file:///fixture"), Err(McpUrlError::Scheme)));
    assert!(matches!(
        McpUrl::validated("https://user:invalid@localhost/mcp"),
        Err(McpUrlError::Credentials)
    ));
    assert!(matches!(
        McpUrl::validated("https://example.com/mcp#fragment"),
        Err(McpUrlError::Fragment)
    ));
    assert_eq!(
        McpUrl::validated("HTTPS://Example.com/mcp")?.as_str(),
        "HTTPS://Example.com/mcp"
    );
    Ok(())
}

/// Every TypeScript config field is sent in declaration order, followed by caller keys verbatim.
#[test]
fn mcp_server_configs_send_every_field_and_extra_keys() -> TestResult {
    let mut tool = McpServerToolPolicy::new("search");
    tool.permission_policy = Some(McpToolPermissionPolicy::AlwaysAsk);
    tool.org_max_permission = Some(McpToolOrgMaxPermission::Blocked);
    tool.extra.insert("future".into(), json!(1));
    let mut remote = McpRemoteServerOptions::new("https://example.com/mcp");
    remote.headers = Some(BTreeMap::from([("Authorization".into(), "Bearer fixture".into())]));
    remote.tools = Some(vec![tool.clone()]);
    remote.timeout = Some(5000);
    remote.always_load = Some(true);
    remote.extra.insert("oauth".into(), json!({"clientId":"fixture"}));
    remote.extra.insert("type".into(), json!("ignored"));
    let stdio = McpStdioServerOptions {
        command: PathBuf::from("server"),
        args: Some(Vec::new()),
        env: Some(trogon_claude_agent_sdk::Environment::default()),
        timeout: Some(1000),
        always_load: Some(false),
        extra: serde_json::from_value(json!({"cwd":"/srv"}))?,
        ..McpStdioServerOptions::default()
    };
    let sdk = McpSdkServerOptions {
        name: "remote-sdk".into(),
        timeout: Some(2000),
        tools: Some(vec![McpServerToolPolicy::new("echo")]),
        extra: serde_json::from_value(json!({"future":true}))?,
    };
    for (server, expected) in [
        (
            McpServerOptions::Http(remote.clone()),
            r#"{"type":"http","url":"https://example.com/mcp","headers":{"Authorization":"Bearer fixture"},"tools":[{"name":"search","permission_policy":"always_ask","org_max_permission":"blocked","future":1}],"timeout":5000,"alwaysLoad":true,"oauth":{"clientId":"fixture"}}"#,
        ),
        (
            McpServerOptions::Sse(remote),
            r#"{"type":"sse","url":"https://example.com/mcp","headers":{"Authorization":"Bearer fixture"},"tools":[{"name":"search","permission_policy":"always_ask","org_max_permission":"blocked","future":1}],"timeout":5000,"alwaysLoad":true,"oauth":{"clientId":"fixture"}}"#,
        ),
        (
            McpServerOptions::Stdio(stdio),
            r#"{"command":"server","args":[],"env":{},"timeout":1000,"alwaysLoad":false,"cwd":"/srv"}"#,
        ),
        (
            McpServerOptions::Stdio(McpStdioServerOptions {
                command: PathBuf::from("server"),
                declare_type: true,
                ..McpStdioServerOptions::default()
            }),
            r#"{"type":"stdio","command":"server"}"#,
        ),
        (
            McpServerOptions::Sdk(sdk),
            r#"{"type":"sdk","name":"remote-sdk","timeout":2000,"tools":[{"name":"echo"}],"future":true}"#,
        ),
    ] {
        assert_eq!(serde_json::to_string(&server.to_wire_json()?)?, expected);
    }
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
                McpServerOptions::Sse(McpRemoteServerOptions::new("https://example.com/events")),
            )])),
        ]);
        let wire = agent.to_wire_json()?;
        assert_eq!(wire["effort"], literal);
        assert_eq!(wire["background"], false);
        assert_eq!(
            wire["mcpServers"],
            json!(["named", {"inline":{"type":"sse","url":"https://example.com/events"}}])
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
    let origin = MessageOrigin::new("host", object(json!({"trace":"one"}))?);
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
        )?),
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
        annotations: None,
        search_hint: None,
        always_load: false,
    };
    assert!(!format!("{options:?}").contains("fixture-private-description"));
    let tool = SdkTool::from(options);
    assert!(!format!("{tool:?}").contains("fixture-private-description"));
    let server_options = LocalMcpServerOptions {
        name: McpServerName::from("registry"),
        version: Some("fixture-private-version".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
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
    // The TypeScript `Protocol` drops what its message schema rejects instead of answering it.
    for request in [
        json!({}),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":u64::MAX,"method":"ping"}),
        json!({"jsonrpc":"1.0","method":"ping"}),
    ] {
        assert!(
            uninitialized
                .request(object(request)?, CancellationToken::new())
                .await?
                .is_none()
        );
    }
    assert_eq!(
        rpc(
            uninitialized.as_ref(),
            json!({"jsonrpc":"2.0","id":9_007_199_254_740_991_u64,"method":"ping"})
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
    assert_eq!(rpc(session.as_ref(), request).await?["error"]["code"], -32603);
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
        input_schema: McpToolInputSchema::new(object(json!({"type":"object"}))?),
        handler: Arc::new(|_request: McpToolRequest| async { Ok::<_, CallbackError>(McpToolOutput::text("ok")) }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("names"),
        version: Some("1".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
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
            -32603,
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
    assert_eq!(
        unknown["result"]["content"][0]["text"],
        "MCP error -32602: Tool unknown not found"
    );
    let invalid = rpc(
        session.as_ref(),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"inspect","arguments":{"status":4}}}),
    )
    .await?;
    assert_eq!(invalid["result"]["isError"], true);
    assert!(invalid["result"]["content"][0]["text"].as_str().is_some_and(|text| {
        text.starts_with("MCP error -32602: Input validation error: Invalid arguments for tool inspect: ")
    }));
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
    let schema = OutputSchema::new(object(json!({"type":"object","description":"fixture-private-schema"}))?);
    assert!(schema.is_valid(&json!({})));
    assert_eq!(format!("{schema:?}"), "OutputSchema { .. }");
    Ok(())
}

#[tokio::test]
async fn s08_local_mcp_discovery_does_not_wait_for_the_initialized_notice_and_supports_empty_registries() -> TestResult
{
    let empty = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("empty"),
        version: Some("1".into()),
        tools: Some(Default::default()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    });
    let (populated, calls) = registry()?;
    // A TypeScript server with no tools never registers the tools handlers.
    let not_found = json!({"code":-32601,"message":"Method not found"});
    for (server, field, expected) in [
        (&empty, "error", not_found),
        (
            &populated,
            "result",
            json!({"tools":[{
                "name":"inspect", "description":"fixture-private-description",
                "inputSchema":{
                    "$schema":"http://json-schema.org/draft-07/schema#",
                    "type":"object","required":["status"],"properties":{"status":{"type":"string"}}
                },
                "execution":{"taskSupport":"forbidden"}
            }]}),
        ),
    ] {
        let session = server.open_session();
        let early = rpc(
            session.as_ref(),
            json!({"jsonrpc":"2.0","id":"early","method":"tools/list"}),
        )
        .await?;
        assert_eq!(early["id"], "early");
        assert_eq!(early[field], expected);
        rpc(session.as_ref(), initialize("2025-11-25")).await?;
        let waiting = rpc(
            session.as_ref(),
            json!({"jsonrpc":"2.0","id":"waiting","method":"tools/list"}),
        )
        .await?;
        assert_eq!(waiting["id"], "waiting");
        assert_eq!(waiting[field], expected);
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
        assert_eq!(listed[field], expected);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

/// `McpServerDialect::Python` answers like the `mcp` server that the Python `create_sdk_mcp_server` builds.
#[tokio::test]
async fn s08_s09_s10_m04_python_dialect_negotiates_gates_and_reads_ids_like_the_mcp_package() -> TestResult {
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("listed"),
        description: "listed".into(),
        input_schema: McpToolInputSchema::new(object(json!({"type":"object","properties":{}}))?),
        handler: Arc::new(|_request: McpToolRequest| async { Ok::<_, CallbackError>(McpToolOutput::text("ok")) }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(tool.into()),
        instructions: Some("ignored by the Python server".into()),
        dialect: McpServerDialect::Python,
        ..LocalMcpServerOptions::new(McpServerName::from("python"))
    });
    let invalid = json!({"code":-32602,"message":"Invalid request parameters","data":""});
    let session = server.open_session();
    let early = rpc(
        session.as_ref(),
        json!({"jsonrpc":"2.0","id":"early","method":"tools/list"}),
    )
    .await?;
    assert_eq!(early["error"], invalid);
    assert_eq!(
        rpc(session.as_ref(), json!({"jsonrpc":"2.0","id":"ping","method":"ping"})).await?["result"],
        json!({})
    );
    let mut request = initialize("2025-11-25");
    request["params"]["clientInfo"] = json!({"name":"contracts"});
    assert_eq!(rpc(session.as_ref(), request).await?["error"], invalid);
    for (requested, negotiated) in [
        ("2024-10-07", "2025-11-25"),
        ("2024-11-05", "2024-11-05"),
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("future-version", "2025-11-25"),
    ] {
        let session = server.open_session();
        let initialized = rpc(session.as_ref(), initialize(requested)).await?;
        assert_eq!(
            initialized["result"],
            json!({
                "capabilities":{"tools":{"listChanged":false}},
                "protocolVersion":negotiated,
                "serverInfo":{"name":"python","version":"1.0.0"}
            })
        );
        // `initialize` alone opens the session; the notice is not required.
        let listed = rpc(
            session.as_ref(),
            json!({"jsonrpc":"2.0","id":"listed","method":"tools/list"}),
        )
        .await?;
        assert_eq!(listed["result"]["tools"][0]["name"], "listed");
    }
    for id in [json!(1.0), json!(1.5), json!(true), Value::Null] {
        let ping = json!({"jsonrpc":"2.0","id":id,"method":"ping"});
        assert!(
            session
                .request(object(ping)?, CancellationToken::new())
                .await?
                .is_none()
        );
    }
    for id in [json!("text"), json!(7), json!(-7), json!(u64::MAX)] {
        let ping = rpc(
            session.as_ref(),
            json!({"jsonrpc":"2.0","id":id.clone(),"method":"ping"}),
        )
        .await?;
        assert_eq!(ping["id"], id);
    }
    Ok(())
}

/// The TypeScript dialect reads a JSON number with a zero fraction as the same integer id, as JavaScript does.
#[tokio::test]
async fn m04_typescript_dialect_reads_string_and_safe_integer_ids() -> TestResult {
    let (server, _calls) = registry()?;
    let session = server.open_session();
    for (id, echoed) in [
        (json!("text"), json!("text")),
        (json!(7), json!(7)),
        (json!(-7), json!(-7)),
        (json!(1.0), json!(1)),
        (json!(9_007_199_254_740_991_u64), json!(9_007_199_254_740_991_u64)),
    ] {
        let ping = rpc(session.as_ref(), json!({"jsonrpc":"2.0","id":id,"method":"ping"})).await?;
        assert_eq!(ping["id"], echoed);
    }
    for id in [json!(1.5), json!(true), Value::Null, json!(9_007_199_254_740_992_u64)] {
        let ping = json!({"jsonrpc":"2.0","id":id,"method":"ping"});
        assert!(
            session
                .request(object(ping)?, CancellationToken::new())
                .await?
                .is_none()
        );
    }
    Ok(())
}
