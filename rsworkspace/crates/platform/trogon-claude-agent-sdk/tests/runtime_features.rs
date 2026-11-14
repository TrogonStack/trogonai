#![cfg(feature = "test-support")]

use serde_json::{Value, json};
use std::{
    error::Error as StdError,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::timeout;
use trogon_claude_agent_sdk::{
    AgentDefinition, AgentName, CancellationToken, ClaudeAgentOptions, ClaudeSdkClient, DialogKind, EffortLevel,
    ElicitationMode, ElicitationRequest, ElicitationResponse, Error, FeatureSupport, FileCheckpointing, HookEvent,
    HookFailureMode, HookOutput, HookPermissionDecision, HookRegistration, HookRequest, HookSpecificOutput, InputBlock,
    InterruptReceipt, LocalMcpServer, LocalMcpServerOptions, LocalPlugin, McpEnabled, McpRemoteServerOptions,
    McpServerDialect, McpServerName, McpServerOptions, McpToolInputSchema, McpToolName, McpToolOutput, McpToolRequest,
    Message, MessageId, OutputSchema, PermissionDecision, PermissionMode, PluginDelivery, Prompt, QueueCapacity,
    RewindMode, SdkTool, SdkToolOptions, SyncHookOutput, TaskId, Timeout, ToolPermissionRequest, ToolUseId, UsdBudget,
    UserDialogRequest, UserDialogResponse, UserInput,
};

type TestResult = Result<(), Box<dyn StdError>>;

fn options(mode: &str) -> Result<ClaudeAgentOptions, Box<dyn StdError>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some(format!("fixture:{mode}").into()),
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(Timeout::new(Duration::from_secs(2)));
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(Timeout::new(Duration::from_millis(200)));
    // The runtime force-kills the fixture on shutdown, possibly while it is writing its coverage
    // profile at exit, which leaves a corrupt profile. The fixture runs no crate code, so its
    // profile is discarded.
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    Ok(options)
}

async fn result(client: &mut ClaudeSdkClient) -> Result<String, Box<dyn StdError>> {
    timeout(Duration::from_secs(5), async {
        while let Some(message) = client.next_message().await? {
            if let Message::Result(result) = message {
                return Ok(result.result.unwrap_or_default());
            }
        }
        Err(std::io::Error::other("fixture ended before result").into())
    })
    .await?
}

#[tokio::test]
async fn typed_controls_preserve_receipts_and_use_exact_wire_operations() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("controls")?).await?;
    let handle = client.handle();
    let receipt = handle.interrupt().await?;
    assert!(
        matches!(receipt,InterruptReceipt::Reported{still_queued,cancelled:None,..} if still_queued[0].as_str()=="queued-one")
    );
    let receipt = handle.interrupt_cancel_queued().await?;
    assert!(
        matches!(receipt,InterruptReceipt::Reported{cancelled:Some(cancelled),..} if cancelled[0].as_str()=="cancelled-one")
    );
    handle.set_model(Some("replacement".into())).await?;
    handle.set_permission_mode(PermissionMode::Plan).await?;
    let rewind = handle
        .rewind_files(MessageId::from("user-message"), RewindMode::DryRun)
        .await?
        .decode()?;
    assert!(rewind.can_rewind);
    assert_eq!(rewind.files_changed, Some(vec!["README.md".to_owned()]));
    assert_eq!(handle.mcp_status().await?.decode()?.mcp_servers[0].name, "local");
    assert_eq!(
        handle
            .context_usage(None)
            .await?
            .as_json()
            .map(|usage| &usage["totalTokens"]),
        Some(&json!(3))
    );
    handle.reconnect_mcp_server(McpServerName::from("local")).await?;
    handle
        .toggle_mcp_server(McpServerName::from("local"), McpEnabled::Disabled)
        .await?;
    handle.stop_task(TaskId::from("task-one")).await?;
    assert!(handle.background_tasks(Some(ToolUseId::from("tool-one"))).await?);
    let requests = timeout(Duration::from_secs(3), async {
        let mut requests = Vec::new();
        while requests.len() < 11 {
            if let Some(Message::System(message)) = client.next_message().await?
                && message.subtype == "control_observed"
            {
                requests.push(message.data["request"].clone());
            }
        }
        Ok::<_, trogon_claude_agent_sdk::Error>(requests)
    })
    .await??;
    assert!(requests.contains(&json!({"subtype":"set_model","model":"replacement"})));
    assert!(requests.contains(&json!({"subtype":"rewind_files","user_message_id":"user-message","dry_run":true})));
    assert!(requests.contains(&json!({"subtype":"mcp_toggle","serverName":"local","enabled":false})));
    assert!(requests.contains(&json!({"subtype":"stop_task","task_id":"task-one"})));
    client.disconnect().await?;
    Ok(())
}

/// TypeScript `readMcpResource` sends `{subtype, serverName, uri}` and resolves with the response as sent.
#[tokio::test]
async fn read_mcp_resource_sends_the_typescript_request_and_returns_the_response() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("controls")?).await?;
    let response = client
        .handle()
        .read_mcp_resource(McpServerName::from("local"), "ui://local/app")
        .await?
        .ok_or("the reply carries a response")?;
    assert_eq!(
        serde_json::to_string(response.as_json())?,
        r#"{"contents":[{"uri":"ui://local/app","mimeType":"text/html;profile=mcp-app","text":"<p>app</p>","_meta":{"ui":{"csp":{}}}}],"extra":true}"#
    );
    let request = timeout(Duration::from_secs(3), async {
        loop {
            if let Some(Message::System(message)) = client.next_message().await?
                && message.subtype == "control_observed"
            {
                return Ok::<_, trogon_claude_agent_sdk::Error>(message.data["request"].clone());
            }
        }
    })
    .await??;
    assert_eq!(
        serde_json::to_string(&request)?,
        r#"{"subtype":"mcp_read_resource","serverName":"local","uri":"ui://local/app"}"#
    );
    client.disconnect().await?;
    Ok(())
}

/// `readMcpResource` returns the reply's `response` field as sent: `undefined` when it is missing, and `null` or
/// any other value unchanged.
#[tokio::test]
async fn read_mcp_resource_returns_none_for_a_reply_without_a_response() -> TestResult {
    let client = ClaudeSdkClient::connect(options("controls")?).await?;
    let missing = client
        .handle()
        .read_mcp_resource(McpServerName::from("local"), "ui://local/none")
        .await?;
    assert_eq!(missing, None);
    let null = client
        .handle()
        .read_mcp_resource(McpServerName::from("local"), "ui://local/null")
        .await?
        .ok_or("a null response is still a response")?;
    assert_eq!(null.as_json(), &Value::Null);
    let text = client
        .handle()
        .read_mcp_resource(McpServerName::from("local"), "ui://local/text")
        .await?
        .ok_or("a text response is still a response")?;
    assert_eq!(serde_json::to_string(text.as_json())?, r#""as sent""#);
    assert_eq!(text.contents(), None);
    client.disconnect().await?;
    Ok(())
}

/// A `readMcpResource` reply with an error subtype rejects the call, as TypeScript rejects its promise.
#[tokio::test]
async fn read_mcp_resource_rejects_when_claude_code_answers_with_an_error() -> TestResult {
    let client = ClaudeSdkClient::connect(options("controls")?).await?;
    let rejected = client
        .handle()
        .read_mcp_resource(McpServerName::from("local"), "https://local/app")
        .await;
    let Err(Error::ControlRejected { operation, rejection }) = &rejected else {
        return Err(format!("expected a rejection, got {rejected:?}").into());
    };
    assert_eq!(*operation, trogon_claude_agent_sdk::ControlOperation::McpReadResource);
    assert_eq!(rejection.message(), "fixture reads ui:// resources only");
    client.disconnect().await?;
    Ok(())
}

fn blocking_hook() -> HookRegistration {
    HookRegistration {
        event: HookEvent::PreToolUse,
        matcher: Some("Bash".into()),
        callbacks: vec![Arc::new(|request: HookRequest| async move {
            assert_eq!(request.tool_use_id.as_ref().map(ToolUseId::as_str), Some("tool-1"));
            assert!(!request.signal.is_cancelled());
            Ok(HookOutput::Sync(Box::new(SyncHookOutput {
                specific: Some(HookSpecificOutput::PreToolUse {
                    permission_decision: Some(HookPermissionDecision::Deny),
                    permission_decision_reason: Some("fixture policy".into()),
                    updated_input: None,
                    additional_context: None,
                }),
                ..SyncHookOutput::default()
            })))
        })],
        timeout: None,
        on_failure: Some(HookFailureMode::Block),
    }
}

#[tokio::test]
async fn blocking_hook_is_registered_and_answered_through_the_public_client() -> TestResult {
    let mut options = options("hook")?;
    options.hooks = vec![blocking_hook()];
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("hook")).await?;
    let output: Value = serde_json::from_str(&result(&mut client).await?)?;
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        output["hookSpecificOutput"]["permissionDecisionReason"],
        "fixture policy"
    );
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn blocking_hook_refuses_a_cli_without_failure_capability() -> TestResult {
    let mut options = options("hook-unsupported")?;
    options.hooks = vec![blocking_hook()];
    assert!(matches!(
        ClaudeSdkClient::connect(options).await,
        Err(Error::BlockingHooksUnsupported)
    ));
    Ok(())
}

#[tokio::test]
async fn embedded_mcp_server_keeps_its_session_across_rpc_calls() -> TestResult {
    let mut options = options("mcp")?;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let schema = McpToolInputSchema::new(serde_json::from_value(
        json!({"type":"object","required":["value"],"properties":{"value":{"type":"string"}}}),
    )?);
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("echo"),
        description: "Echo".into(),
        input_schema: schema,
        handler: Arc::new(move |request: McpToolRequest| {
            let calls = Arc::clone(&observed);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(McpToolOutput::text(
                    request.arguments["value"].as_str().unwrap_or_default(),
                ))
            }
        }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let server = LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from("local"),
        version: Some("1".into()),
        tools: Some(tool.into()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    });
    options
        .sdk_mcp_servers
        .insert(McpServerName::from("local"), Arc::new(server));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("mcp")).await?;
    assert_eq!(result(&mut client).await?, "from-fixture");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn structured_input_and_declarative_configuration_reach_the_cli() -> TestResult {
    let mut options = options("structured")?;
    options.max_budget_usd = Some(UsdBudget::new(0.25));
    options.effort = Some(EffortLevel::High);
    options.file_checkpointing = FileCheckpointing::Enabled;
    options.output_schema = Some(OutputSchema::new(serde_json::from_value(
        json!({"type":"object","properties":{"ok":{"type":"boolean"}}}),
    )?));
    options.agents.insert(
        AgentName::from("reviewer"),
        AgentDefinition::new("Review", Prompt::from("Review the code")),
    );
    options.mcp_servers.insert(
        McpServerName::from("external"),
        McpServerOptions::Http(McpRemoteServerOptions::new("https://example.com/mcp")),
    );
    let mut client = ClaudeSdkClient::connect(options).await?;
    let Some(Message::System(init)) = client.next_message().await? else {
        return Err(std::io::Error::other("missing init").into());
    };
    let arguments = init.data["arguments"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("missing argv"))?;
    assert!(arguments.iter().any(|value| value == "--max-budget-usd=0.25"));
    assert!(arguments.iter().any(|value| value == "--effort=high"));
    assert!(
        arguments
            .iter()
            .any(|value| value.as_str().is_some_and(|value| value.starts_with("--json-schema=")))
    );
    // Upstream TypeScript passes `--mcp-config` and its value as separate tokens.
    assert!(arguments.iter().any(|value| value == "--mcp-config"));
    assert_eq!(
        init.data["initialize_request"]["agents"]["reviewer"]["prompt"],
        "Review the code"
    );
    let mut input = UserInput::blocks(vec![
        InputBlock::text("inspect"),
        InputBlock::from_json(serde_json::from_value(
            json!({"type":"image","source":{"type":"url","url":"https://example.com/image.png"}}),
        )?),
    ]);
    input.uuid = Some(MessageId::from("structured-user"));
    client.send_input(input).await?;
    let Some(Message::System(observed)) = client.next_message().await? else {
        return Err(std::io::Error::other("missing input receipt").into());
    };
    assert_eq!(observed.subtype, "input_observed");
    assert_eq!(observed.data["input"]["uuid"], "structured-user");
    assert_eq!(observed.data["input"]["message"]["content"][1]["type"], "image");
    assert_eq!(result(&mut client).await?, "structured");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn plugins_reach_the_cli_by_argv_or_by_the_initialize_request() -> TestResult {
    for delivery in [PluginDelivery::Argv, PluginDelivery::Initialize] {
        let mut options = options("structured")?;
        options.plugin_delivery = delivery;
        options.plugins = vec![
            LocalPlugin::new("/plugins/one"),
            LocalPlugin {
                path: "/plugins/two".into(),
                skip_mcp_discovery: Some(true),
            },
        ];
        let mut client = ClaudeSdkClient::connect(options).await?;
        let Some(Message::System(init)) = client.next_message().await? else {
            return Err(std::io::Error::other("missing init").into());
        };
        let arguments = init.data["arguments"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("missing argv"))?;
        let has = |token: &str| arguments.iter().any(|value| value == token);
        match delivery {
            PluginDelivery::Argv => {
                assert!(has("--plugin-dir=/plugins/one"));
                assert!(has("--plugin-dir-no-mcp=/plugins/two"));
                assert!(!has("--await-initialize"));
                assert!(init.data["initialize_request"].get("plugins").is_none());
            }
            PluginDelivery::Initialize => {
                assert!(has("--await-initialize"));
                assert!(
                    !arguments
                        .iter()
                        .any(|value| value.as_str().is_some_and(|value| value.starts_with("--plugin-dir")))
                );
                assert_eq!(
                    init.data["initialize_request"]["plugins"],
                    json!([
                        {"type": "local", "path": "/plugins/one"},
                        {"type": "local", "path": "/plugins/two", "skipMcpDiscovery": true}
                    ])
                );
            }
        }
        client.disconnect().await?;
    }
    Ok(())
}

#[tokio::test]
async fn permission_context_and_updates_round_trip_through_the_callback() -> TestResult {
    for (mode, destination) in [("permission-context", "session"), ("permission-cli-arg", "cliArg")] {
        let mut options = options(mode)?;
        options.permission_handler = Some(Arc::new(|request: ToolPermissionRequest| async move {
            assert_eq!(request.request_id.as_str(), "permission-context");
            assert_eq!(
                request.tool_use_id.as_ref().map(ToolUseId::as_str),
                Some("tool-context")
            );
            assert_eq!(request.agent_id.as_deref(), Some("reviewer"));
            assert_eq!(
                request.mcp_server.as_ref().map(|server| server.source.as_str()),
                Some("sdk")
            );
            assert_eq!(request.title.as_deref(), Some("Permission title"));
            assert_eq!(request.display_name.as_deref(), Some("Read file"));
            assert_eq!(request.description.as_deref(), Some("Permission detail"));
            assert_eq!(request.default_to_no, FeatureSupport::Supported);
            assert_eq!(request.suppress_always_allow_rule, FeatureSupport::Supported);
            assert_eq!(request.requires_user_interaction, FeatureSupport::Supported);
            assert!(!request.signal.is_cancelled());
            assert_eq!(request.suggestions.len(), 1);
            PermissionDecision::Allow {
                updated_input: None,
                updated_permissions: Some(request.suggestions),
                decision_classification: None,
            }
        }));
        let mut client = ClaudeSdkClient::connect(options).await?;
        client.send_prompt(Prompt::from("context")).await?;
        let update = timeout(Duration::from_secs(3), async {
            while let Some(message) = client.next_message().await? {
                if let Message::System(message) = message
                    && message.subtype == "permission_observed"
                {
                    return Ok::<_, Box<dyn StdError>>(message.data["response"].clone());
                }
            }
            Err(std::io::Error::other("missing permission receipt").into())
        })
        .await??;
        assert_eq!(update["behavior"], "allow");
        assert_eq!(update["updatedPermissions"][0]["type"], "addRules");
        assert_eq!(update["updatedPermissions"][0]["destination"], destination);
        assert_eq!(result(&mut client).await?, "context");
        client.disconnect().await?;
    }
    Ok(())
}

#[tokio::test]
async fn cancelled_permission_exposes_its_signal_to_the_host() -> TestResult {
    let mut options = options("permission-signal")?;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::new(std::sync::Mutex::new(None::<CancellationToken>));
    let observed_signal = Arc::clone(&signal);
    let observed_started = Arc::clone(&started);
    options.permission_handler = Some(Arc::new(move |request: ToolPermissionRequest| {
        if let Ok(mut signal) = observed_signal.lock() {
            *signal = Some(request.signal);
        }
        observed_started.notify_one();
        async { std::future::pending::<PermissionDecision>().await }
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("cancel")).await?;
    timeout(Duration::from_secs(3), started.notified()).await?;
    let token = signal
        .lock()
        .map_err(|_| std::io::Error::other("signal lock poisoned"))?
        .clone()
        .ok_or_else(|| std::io::Error::other("missing callback signal"))?;
    client.handle().interrupt().await?;
    timeout(Duration::from_secs(3), token.cancelled()).await?;
    assert_eq!(result(&mut client).await?, "cancelled");
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn blocking_hook_refuses_a_session_that_did_not_apply_it() -> TestResult {
    let mut options = options("hook-not-applied")?;
    options.hooks = vec![blocking_hook()];
    assert!(matches!(
        ClaudeSdkClient::connect(options).await,
        Err(Error::HooksNotApplied)
    ));
    Ok(())
}

#[tokio::test]
async fn elicitation_callback_returns_a_typed_form_response() -> TestResult {
    let mut options = options("elicitation")?;
    options.elicitation_handler = Some(Arc::new(|request: ElicitationRequest| async move {
        assert_eq!(request.server_name.as_str(), "local");
        assert_eq!(request.mode, Some(ElicitationMode::Form));
        assert!(request.requested_schema.is_some());
        assert!(!request.signal.is_cancelled());
        let mut content = trogon_claude_agent_sdk::JsonObject::new();
        content.insert("approved".into(), true.into());
        Ok(ElicitationResponse::Accept { content: Some(content) })
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("elicit")).await?;
    let response: Value = serde_json::from_str(&result(&mut client).await?)?;
    assert_eq!(response["action"], "accept");
    assert_eq!(response["content"]["approved"], true);
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn declared_user_dialog_calls_the_handler() -> TestResult {
    let mut options = options("dialog")?;
    options.supported_dialog_kinds = vec![DialogKind::from("fixture-dialog")];
    options.user_dialog_handler = Some(Arc::new(|request: UserDialogRequest| async move {
        assert_eq!(request.dialog_kind.as_str(), "fixture-dialog");
        assert_eq!(request.payload["choice"], 1);
        assert!(!request.signal.is_cancelled());
        Ok(UserDialogResponse::Completed {
            result: json!({"selected":1}),
        })
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("dialog")).await?;
    let response: Value = serde_json::from_str(&result(&mut client).await?)?;
    assert_eq!(response["behavior"], "completed");
    assert_eq!(response["result"]["selected"], 1);
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn an_undeclared_dialog_kind_still_reaches_the_handler_like_the_typescript_sdk() -> TestResult {
    let mut options = options("dialog-undeclared")?;
    options.supported_dialog_kinds = vec![DialogKind::from("different-kind")];
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    options.user_dialog_handler = Some(Arc::new(move |request: UserDialogRequest| {
        observed.fetch_add(1, Ordering::SeqCst);
        async move {
            assert_eq!(request.dialog_kind.as_str(), "undeclared");
            assert_eq!(request.request_id.as_str(), "dialog-1");
            Ok(UserDialogResponse::Cancelled)
        }
    }));
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("dialog")).await?;
    let response: Value = serde_json::from_str(&result(&mut client).await?)?;
    assert_eq!(response, json!({"behavior":"cancelled"}));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    client.disconnect().await?;
    Ok(())
}

async fn unanswered_dialog_with_pending_callback(mode: &str, handler: bool) -> TestResult {
    let mut options = options(mode)?;
    options.limits.max_incoming_callbacks = Some(QueueCapacity::new(1)?);
    options.permission_handler = Some(Arc::new(|_: ToolPermissionRequest| {
        std::future::pending::<PermissionDecision>()
    }));
    let calls = Arc::new(AtomicUsize::new(0));
    if handler {
        options.supported_dialog_kinds = vec![DialogKind::from("different-kind")];
        let observed = Arc::clone(&calls);
        options.user_dialog_handler = Some(Arc::new(move |_: UserDialogRequest| {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Ok(UserDialogResponse::Cancelled) }
        }));
    }
    let mut client = ClaudeSdkClient::connect(options).await?;
    client.send_prompt(Prompt::from("dialog")).await?;
    assert_eq!(result(&mut client).await?, "undeclared");
    client.handle().interrupt().await?;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn saturated_callbacks_leave_a_dialog_without_a_handler_unanswered() -> TestResult {
    unanswered_dialog_with_pending_callback("dialog-undeclared-saturated", false).await
}

#[tokio::test]
async fn a_dialog_reusing_an_in_flight_id_is_skipped() -> TestResult {
    unanswered_dialog_with_pending_callback("dialog-undeclared-duplicate", true).await
}

#[tokio::test]
async fn s06_continue_hooks_tolerate_a_session_that_did_not_apply_them() -> TestResult {
    let mut options = options("hook-not-applied")?;
    let mut hook = blocking_hook();
    hook.on_failure = Some(HookFailureMode::Continue);
    options.hooks = vec![hook];
    let client = ClaudeSdkClient::connect(options).await?;
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn continue_hook_does_not_require_blocking_failure_capability() -> TestResult {
    let mut options = options("hook-unsupported")?;
    let mut hook = blocking_hook();
    hook.on_failure = Some(HookFailureMode::Continue);
    options.hooks = vec![hook];
    let client = ClaudeSdkClient::connect(options).await?;
    client.disconnect().await?;
    Ok(())
}
