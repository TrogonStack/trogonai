#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Public protocol tests exchange exact scripted CLI frames."
    )
)]

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines},
    time::timeout,
};
use trogon_claude_agent_sdk::{
    AgentName, CallbackError, CancellationToken, ClaudeAgentOptions, ClaudeSdkClient, Content, ContentBlock,
    ContinueConversation, Customizations, DialogKind, ElicitationMode, ElicitationRequest, ElicitationResponse, Error,
    HookEvent, HookFailureMode, HookRegistration, HookRequest, JsonObject, McpFuture, McpServerName, McpServerOptions,
    McpUrl, Message, MessageId, PermissionDecision, Prompt, ProtocolError, QueueCapacity, ResultStatus, SdkMcpServer,
    SdkMcpSession, SessionEntry, SessionError, SessionFork, SessionKey, SessionPersistence, SessionProjectKey,
    SessionStore, SessionStoreError, SessionStoreFuture, SessionStoreOptions, SystemPrompt, Timeout,
    ToolPermissionRequest, ToolResultStatus, UserDialogRequest, UserInput,
    adapters::{NativeFileSystem, RuntimeAdapters, SpawnProcess, TokioProcessSpawner},
    query_turn_with_adapters, query_with_adapters,
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessEvent, ScriptedProcessHarness, ScriptedProcessObserver, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn options() -> Result<ClaudeAgentOptions, Box<dyn std::error::Error>> {
    let mut options = ClaudeAgentOptions::default();
    options.limits.control_timeout = Timeout::new(Duration::from_secs(1))?;
    options.limits.shutdown_timeout = Timeout::new(Duration::from_secs(5))?;
    Ok(options)
}

fn adapters<P>(process: P) -> RuntimeAdapters<FixedEnvironment, P> {
    RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-project"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    }
}

struct ScriptedCli {
    lines: Lines<BufReader<DuplexStream>>,
    stdout: DuplexStream,
    stderr: DuplexStream,
    observer: ScriptedProcessObserver,
}

impl From<ScriptedProcessHarness> for ScriptedCli {
    fn from(harness: ScriptedProcessHarness) -> Self {
        Self {
            lines: BufReader::new(harness.stdin).lines(),
            stdout: harness.stdout,
            stderr: harness.stderr,
            observer: harness.observer,
        }
    }
}

impl ScriptedCli {
    async fn read(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        let line = timeout(Duration::from_secs(2), self.lines.next_line())
            .await??
            .ok_or_else(|| io::Error::other("expected a CLI frame"))?;
        Ok(serde_json::from_str(&line)?)
    }

    async fn send(&mut self, frame: Value) -> TestResult {
        let mut bytes = serde_json::to_vec(&frame)?;
        bytes.push(b'\n');
        self.stdout.write_all(&bytes).await?;
        Ok(())
    }

    async fn success(&mut self, request: &Value, response: Value) -> TestResult {
        self.send(json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":response}})).await
    }

    fn end(self, status: i32) {
        self.observer.complete(status);
        drop(self.stdout);
        drop(self.stderr);
    }

    async fn disconnect(mut self, client: ClaudeSdkClient) -> TestResult {
        let closing = tokio::spawn(client.disconnect());
        assert!(
            timeout(Duration::from_secs(2), self.lines.next_line())
                .await??
                .is_none()
        );
        self.end(0);
        closing.await??;
        Ok(())
    }
}

async fn connect(options: ClaudeAgentOptions) -> Result<(ClaudeSdkClient, ScriptedCli), Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let mut cli = ScriptedCli::from(harness);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_adapters(options, adapters(process)));
    let request = cli.read().await?;
    assert_eq!(request["request"]["subtype"], "initialize");
    cli.success(&request, json!({"hooks_applied":true,"hook_on_failure_supported":true}))
        .await?;
    Ok((connecting.await??, cli))
}

fn result() -> Value {
    json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"session_id":"memory-session","result":"complete","queued_turn_count":0})
}

async fn terminal(client: &mut ClaudeSdkClient) -> Result<Error, Box<dyn std::error::Error>> {
    timeout(Duration::from_secs(2), async {
        loop {
            match client.next_message().await {
                Err(error) => return Ok(error),
                Ok(Some(_)) => {}
                Ok(None) => return Err(io::Error::other("expected a terminal SDK error").into()),
            }
        }
    })
    .await?
}

#[tokio::test]
async fn user_and_assistant_blocks_preserve_wire_content_across_line_endings() -> TestResult {
    let (mut client, mut cli) = connect(options()?).await?;
    cli.stdout.write_all(b"\n\r\n").await?;
    let blocks = json!([
        {"type":"thinking","thinking":"reason","signature":"signature"},
        {"type":"tool_use","id":"tool-one","name":"Read","input":{"path":"file"}},
        {"type":"tool_result","tool_use_id":"tool-one","content":"failed","is_error":true},
        {"type":"tool_result","tool_use_id":"tool-one","content":"worked","is_error":false},
        {"type":"tool_result","tool_use_id":"tool-one","content":"unspecified"},
        {"type":"future-block","extra":7}
    ]);
    cli.send(json!({"type":"assistant","message":{"model":"fixture","content":blocks}}))
        .await?;
    let Some(Message::Assistant(assistant)) = client.next_message().await? else {
        return Err(io::Error::other("missing assistant frame").into());
    };
    assert!(
        matches!(&assistant.content[0], ContentBlock::Thinking { thinking, signature } if thinking == "reason" && signature == "signature")
    );
    assert!(
        matches!(&assistant.content[1], ContentBlock::ToolUse { id, name, input } if id.as_str() == "tool-one" && name.as_str() == "Read" && input["path"] == "file")
    );
    for (index, expected) in [
        (2, ToolResultStatus::Error),
        (3, ToolResultStatus::Success),
        (4, ToolResultStatus::NotReported),
    ] {
        assert!(matches!(&assistant.content[index], ContentBlock::ToolResult { status, .. } if *status == expected));
    }
    assert!(
        matches!(&assistant.content[5], ContentBlock::Unknown { kind, data } if kind == "future-block" && data["extra"] == 7)
    );
    let user = json!({"type":"user","uuid":"user-one","session_id":"memory-session","parent_tool_use_id":"parent-one","origin":{"kind":"fixture","host":"memory"},"tool_use_result":{"retained":true},"message":{"role":"user","content":blocks,"nested_extra":1},"outer_extra":2});
    let bytes = format!("{user}\r\n");
    cli.stdout.write_all(bytes.as_bytes()).await?;
    let Some(Message::User(user)) = client.next_message().await? else {
        return Err(io::Error::other("missing structured user frame").into());
    };
    assert!(matches!(&user.content, Content::Blocks(blocks) if blocks.len() == 6));
    assert_eq!(user.uuid.as_ref().map(|id| id.as_str()), Some("user-one"));
    assert_eq!(
        user.parent_tool_use_id.as_ref().map(|id| id.as_str()),
        Some("parent-one")
    );
    assert_eq!(user.origin.as_ref().map(|origin| origin.kind()), Some("fixture"));
    assert_eq!(user.additional_fields["outer_extra"], 2);
    assert_eq!(user.message_fields["nested_extra"], 1);
    assert_eq!(
        user.tool_use_result.as_ref().map(|value| &value["retained"]),
        Some(&Value::Bool(true))
    );
    cli.stdout
        .write_all(b"{\"type\":\"user\",\"message\":{\"content\":\"trailing frame\"}}")
        .await?;
    cli.end(0);
    assert!(
        matches!(client.next_message().await?, Some(Message::User(user)) if matches!(&user.content, Content::Text(text) if text == "trailing frame"))
    );
    assert!(client.next_message().await?.is_none());
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn declarative_cli_choices_preserve_named_arguments() -> TestResult {
    for system in [
        SystemPrompt::ClaudeCode,
        SystemPrompt::ClaudeCodeWithAppend("extra context".into()),
    ] {
        let mut configured = options()?;
        configured.system_prompt = system.clone();
        configured.agent = Some(AgentName::from("reviewer"));
        configured.max_turns = NonZeroUsize::new(3);
        configured.continue_conversation = ContinueConversation::Yes;
        configured.fork_session = SessionFork::Fork;
        configured.session_persistence = SessionPersistence::Disabled;
        configured.customizations = Customizations::Bare;
        configured.additional_directories = vec![PathBuf::from("folder with spaces")];
        let (client, cli) = connect(configured).await?;
        let commands = cli.observer.commands()?;
        let arguments = &commands[0].arguments;
        // The default TypeScript argv shape uses `--flag=value` tokens.
        for expected in [
            "--agent=reviewer",
            "--max-turns=3",
            "--continue",
            "--fork-session",
            "--no-session-persistence",
            "--bare",
            "--add-dir=folder with spaces",
        ] {
            assert!(
                arguments.iter().any(|argument| argument == expected),
                "{expected} missing from {arguments:?}"
            );
        }
        // The TypeScript shape delivers the system prompt through `initialize`, never on argv.
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.to_string_lossy().contains("system-prompt"))
        );
        cli.disconnect(client).await?;
    }
    Ok(())
}

#[tokio::test]
/// Neither upstream SDK caps hook callbacks, so registering more callbacks than pending controls still connects.
async fn s13_empty_hooks_are_ignored_and_hook_callback_count_is_unbounded() -> TestResult {
    let registration = HookRegistration {
        event: HookEvent::PreToolUse,
        matcher: None,
        callbacks: vec![],
        timeout: None,
        on_failure: Some(HookFailureMode::Continue),
    };
    let mut configured = options()?;
    configured.hooks = vec![registration.clone()];
    let (client, cli) = connect(configured).await?;
    cli.disconnect(client).await?;
    let mut configured = options()?;
    configured.limits.max_pending_controls = QueueCapacity::new(1)?;
    let mut registration = registration;
    registration.callbacks = vec![
        Arc::new(|_: HookRequest| async { Err(CallbackError::Cancelled) }),
        Arc::new(|_: HookRequest| async { Err(CallbackError::Cancelled) }),
    ];
    configured.hooks = vec![registration];
    let (client, cli) = connect(configured).await?;
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
/// Both upstream SDKs write input whenever it arrives, so a prompt sent mid-turn reaches the CLI.
async fn s15_input_sent_during_a_turn_is_written_and_closed_input_stays_recoverable() -> TestResult {
    let (mut client, mut cli) = connect(options()?).await?;
    client.query(Prompt::from("first")).await?;
    assert_eq!(cli.read().await?["message"]["content"], "first");
    client.send_prompt(Prompt::from("during the turn")).await?;
    assert_eq!(cli.read().await?["message"]["content"], "during the turn");
    cli.send(result()).await?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
    client.handle().send_prompt(Prompt::from("second")).await?;
    assert_eq!(cli.read().await?["message"]["content"], "second");
    cli.send(result()).await?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
    client
        .stream_input(futures_util::stream::iter([
            UserInput::from(Prompt::from("third")),
            UserInput::from(Prompt::from("fourth")),
        ]))
        .await?;
    assert_eq!(cli.read().await?["message"]["content"], "third");
    assert_eq!(cli.read().await?["message"]["content"], "fourth");
    client.finish_input().await?;
    assert!(matches!(
        client.send_prompt(Prompt::from("closed")).await,
        Err(Error::InputClosed)
    ));
    cli.send(result()).await?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
    cli.end(0);
    assert!(client.next_message().await?.is_none());
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn query_facades_expose_handles_and_cleanup_after_completion_or_early_close() -> TestResult {
    for one_turn in [false, true] {
        let (process, harness) = ScriptedProcessSpawner::new();
        let mut cli = ScriptedCli::from(harness);
        let configured = options()?;
        let prompt = Prompt::from("query");
        let query = tokio::spawn(async move {
            if one_turn {
                query_turn_with_adapters(prompt, configured, adapters(process)).await
            } else {
                query_with_adapters(prompt, configured, adapters(process)).await
            }
        });
        let request = cli.read().await?;
        cli.success(&request, json!({})).await?;
        cli.read().await?;
        let mut query = query.await??;
        assert!(query.handle().is_some());
        if one_turn {
            cli.send(result()).await?;
            cli.end(0);
            assert!(matches!(query.next_message().await?, Some(Message::Result(_))));
            assert!(query.next_message().await?.is_none());
            assert!(query.next_message().await?.is_none());
            assert!(query.handle().is_none());
            query.close().await?;
        } else {
            let closing = tokio::spawn(query.close());
            assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
            cli.end(0);
            closing.await??;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CallbackCase {
    UnknownHook,
    FailedHook,
    UnknownMcp,
    FailedMcp,
    MissingElicitation,
    InvalidElicitation,
    UrlElicitation,
    FailedDialog,
}

struct FailedMcp;
impl SdkMcpServer for FailedMcp {
    fn open_session(&self) -> Arc<dyn SdkMcpSession> {
        Arc::new(Self)
    }
}
impl SdkMcpSession for FailedMcp {
    fn request(&self, _: JsonObject, _: CancellationToken) -> McpFuture<'_> {
        Box::pin(async { Err(CallbackError::Cancelled.into()) })
    }
}

#[tokio::test]
async fn callback_failures_return_bounded_protocol_fallbacks_without_stopping_controls() -> TestResult {
    for case in [
        CallbackCase::UnknownHook,
        CallbackCase::FailedHook,
        CallbackCase::UnknownMcp,
        CallbackCase::FailedMcp,
        CallbackCase::MissingElicitation,
        CallbackCase::InvalidElicitation,
        CallbackCase::UrlElicitation,
        CallbackCase::FailedDialog,
    ] {
        let mut configured = options()?;
        let (request, expected) = match case {
            CallbackCase::UnknownHook => (
                json!({"subtype":"hook_callback","callback_id":"missing","input":{}}),
                "error",
            ),
            CallbackCase::FailedHook => {
                configured.hooks = vec![HookRegistration {
                    event: HookEvent::PreToolUse,
                    matcher: None,
                    callbacks: vec![Arc::new(|_: HookRequest| async { Err(CallbackError::Cancelled) })],
                    timeout: None,
                    on_failure: Some(HookFailureMode::Continue),
                }];
                (
                    json!({"subtype":"hook_callback","callback_id":"hook_0","input":{}}),
                    "error",
                )
            }
            CallbackCase::UnknownMcp => (
                json!({"subtype":"mcp_message","server_name":"missing","message":{}}),
                "error",
            ),
            CallbackCase::FailedMcp => {
                configured
                    .sdk_mcp_servers
                    .insert(McpServerName::from("local"), Arc::new(FailedMcp));
                (
                    json!({"subtype":"mcp_message","server_name":"local","message":{}}),
                    "error",
                )
            }
            CallbackCase::MissingElicitation => (
                json!({"subtype":"elicitation","mcp_server_name":"local","message":"request","mode":"form"}),
                "success",
            ),
            CallbackCase::InvalidElicitation => {
                configured.elicitation_handler = Some(Arc::new(|_: ElicitationRequest| async {
                    Ok(ElicitationResponse::Decline)
                }));
                (
                    json!({"subtype":"elicitation","mcp_server_name":"local","message":"request","mode":"future"}),
                    "error",
                )
            }
            CallbackCase::UrlElicitation => {
                configured.elicitation_handler = Some(Arc::new(|request: ElicitationRequest| async move {
                    assert_eq!(request.mode, ElicitationMode::Url);
                    assert_eq!(request.url.as_deref(), Some("https://example.com/approval"));
                    Err(CallbackError::Cancelled)
                }));
                (
                    json!({"subtype":"elicitation","mcp_server_name":"local","message":"request","mode":"url","url":"https://example.com/approval"}),
                    "success",
                )
            }
            CallbackCase::FailedDialog => {
                configured.supported_dialog_kinds = vec![DialogKind::from("fixture")];
                configured.user_dialog_handler =
                    Some(Arc::new(|_: UserDialogRequest| async { Err(CallbackError::Cancelled) }));
                (
                    json!({"subtype":"request_user_dialog","dialog_kind":"fixture","payload":{}}),
                    "success",
                )
            }
        };
        let (client, mut cli) = connect(configured).await?;
        cli.send(json!({"type":"control_request","request_id":"callback-one","request":request}))
            .await?;
        let response = cli.read().await?;
        assert_eq!(response["response"]["request_id"], "callback-one");
        assert_eq!(response["response"]["subtype"], expected);
        if matches!(case, CallbackCase::MissingElicitation | CallbackCase::UrlElicitation) {
            assert_eq!(response["response"]["response"]["action"], "cancel");
        }
        if matches!(case, CallbackCase::FailedDialog) {
            assert_eq!(response["response"]["response"]["behavior"], "cancelled");
        }
        let handle = client.handle();
        let control = tokio::spawn(async move { handle.set_model(None).await });
        let request = cli.read().await?;
        assert_eq!(request["request"]["subtype"], "set_model");
        cli.success(&request, json!({})).await?;
        control.await??;
        cli.disconnect(client).await?;
    }
    Ok(())
}

#[tokio::test]
async fn pending_controls_and_mcp_replacements_reject_overlapping_admission() -> TestResult {
    let mut configured = options()?;
    configured.limits.max_pending_controls = QueueCapacity::new(1)?;
    let (client, mut cli) = connect(configured).await?;
    let handle = client.handle();
    let pending = tokio::spawn(async move { handle.set_model(None).await });
    let request = cli.read().await?;
    assert!(matches!(
        client.handle().set_model(None).await,
        Err(Error::TooManyControls)
    ));
    assert!(matches!(
        client.handle().set_mcp_servers(BTreeMap::new(), BTreeMap::new()).await,
        Err(Error::TooManyControls)
    ));
    cli.success(&request, json!({})).await?;
    pending.await??;
    let handle = client.handle();
    let replacing = tokio::spawn(async move { handle.set_mcp_servers(BTreeMap::new(), BTreeMap::new()).await });
    let request = cli.read().await?;
    assert!(matches!(
        client.handle().set_mcp_servers(BTreeMap::new(), BTreeMap::new()).await,
        Err(Error::McpUpdateInProgress)
    ));
    cli.success(&request, json!({"added":[],"removed":[],"errors":{}}))
        .await?;
    replacing.await??;
    let name = McpServerName::from("duplicate");
    let external = BTreeMap::from([(
        name.clone(),
        McpServerOptions::Http {
            url: McpUrl::new("https://example.com/mcp")?,
            headers: BTreeMap::new(),
        },
    )]);
    let embedded: BTreeMap<McpServerName, Arc<dyn SdkMcpServer>> =
        BTreeMap::from([(name, Arc::new(FailedMcp) as Arc<dyn SdkMcpServer>)]);
    assert!(matches!(
        client.handle().set_mcp_servers(external, embedded).await,
        Err(Error::DuplicateMcpServer)
    ));
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
async fn partial_mcp_errors_preserve_previous_server_and_omit_failed_new_server() -> TestResult {
    let mut configured = options()?;
    configured
        .sdk_mcp_servers
        .insert(McpServerName::from("previous"), Arc::new(FailedMcp));
    let (client, mut cli) = connect(configured).await?;
    let embedded: BTreeMap<McpServerName, Arc<dyn SdkMcpServer>> = BTreeMap::from([(
        McpServerName::from("fresh"),
        Arc::new(FailedMcp) as Arc<dyn SdkMcpServer>,
    )]);
    let handle = client.handle();
    let replacing = tokio::spawn(async move { handle.set_mcp_servers(BTreeMap::new(), embedded).await });
    let request = cli.read().await?;
    cli.success(
        &request,
        json!({"added":[],"removed":[],"errors":{"previous":"retain previous","fresh":"reject fresh"}}),
    )
    .await?;
    assert_eq!(replacing.await??.errors.len(), 2);
    for (name, expected) in [
        ("previous", "SDK MCP request failed"),
        ("fresh", "Unknown SDK MCP server"),
    ] {
        cli.send(json!({"type":"control_request","request_id":"lookup","request":{"subtype":"mcp_message","server_name":name,"message":{}}})).await?;
        let response = cli.read().await?;
        assert_eq!(response["response"]["subtype"], "error");
        assert_eq!(response["response"]["error"], expected);
    }
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
async fn callback_timeout_cancels_signal_and_returns_deny() -> TestResult {
    let signal = Arc::new(std::sync::Mutex::new(None::<CancellationToken>));
    let observed = signal.clone();
    let mut configured = options()?;
    configured.permission_handler = Some(Arc::new(move |request: ToolPermissionRequest| {
        if let Ok(mut signal) = observed.lock() {
            *signal = Some(request.signal);
        }
        async { std::future::pending::<PermissionDecision>().await }
    }));
    let (client, mut cli) = connect(configured).await?;
    cli.send(json!({"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}})).await?;
    let response = cli.read().await?;
    assert_eq!(response["response"]["response"]["behavior"], "deny");
    let signal = signal
        .lock()
        .map_err(|_| io::Error::other("signal lock poisoned"))?
        .clone()
        .ok_or_else(|| io::Error::other("missing callback signal"))?;
    assert!(signal.is_cancelled());
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
async fn callback_capacity_is_bounded_and_cancellation_preserves_control_progress() -> TestResult {
    let started = Arc::new(tokio::sync::Notify::new());
    let observed = started.clone();
    let mut configured = options()?;
    configured.limits.max_pending_controls = QueueCapacity::new(1)?;
    configured.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        observed.notify_one();
        async { std::future::pending::<PermissionDecision>().await }
    }));
    configured.supported_dialog_kinds = vec![DialogKind::from("fixture")];
    configured.user_dialog_handler = Some(Arc::new(|_: UserDialogRequest| async { Err(CallbackError::Cancelled) }));
    let (client, mut cli) = connect(configured).await?;
    cli.send(json!({"type":"control_request","request_id":"pending","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}})).await?;
    started.notified().await;
    for request in [
        json!({"subtype":"future_callback"}),
        json!({"subtype":"can_use_tool","tool_name":"Read","input":{}}),
        json!({"subtype":"can_use_tool","input":{}}),
        json!({"subtype":"hook_callback","callback_id":"missing","input":{}}),
        json!({"subtype":"mcp_message","server_name":"missing","message":{}}),
        json!({"subtype":"elicitation","mcp_server_name":"local","message":"request","mode":"form"}),
        json!({"subtype":"request_user_dialog","dialog_kind":"fixture","payload":{}}),
    ] {
        cli.send(json!({"type":"control_request","request_id":"excess","request":request}))
            .await?;
        let response = cli.read().await?;
        assert_eq!(response["response"]["request_id"], "excess");
        assert_eq!(response["response"]["error"], "Too many outstanding SDK callbacks");
    }
    cli.send(json!({"type":"control_cancel_request","request_id":"pending"}))
        .await?;
    cli.send(json!({"type":"control_request","request_id":"after-cancel","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}})).await?;
    let handle = client.handle();
    let control = tokio::spawn(async move { handle.set_model(None).await });
    let request = cli.read().await?;
    assert_eq!(request["request"]["subtype"], "set_model");
    cli.success(&request, json!({})).await?;
    control.await??;
    started.notified().await;
    cli.send(json!({"type":"control_cancel_request","request_id":"after-cancel"}))
        .await?;
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
async fn duplicate_callback_request_id_is_a_terminal_protocol_error() -> TestResult {
    let started = Arc::new(tokio::sync::Notify::new());
    let observed = started.clone();
    let mut configured = options()?;
    configured.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        observed.notify_one();
        async { std::future::pending::<PermissionDecision>().await }
    }));
    let (mut client, mut cli) = connect(configured).await?;
    let request = json!({"type":"control_request","request_id":"duplicate","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}});
    cli.send(request.clone()).await?;
    started.notified().await;
    cli.send(request).await?;
    cli.end(0);
    assert!(matches!(
        terminal(&mut client).await?,
        Error::Protocol(ProtocolError::DuplicateControlRequest)
    ));
    Ok(())
}

#[tokio::test]
async fn an_impossible_queued_turn_count_fails_through_a_typed_public_error() -> TestResult {
    let (mut client, mut cli) = connect(options()?).await?;
    let mut frame = result();
    frame["queued_turn_count"] = u64::MAX.into();
    cli.send(frame).await?;
    cli.end(0);
    assert!(matches!(terminal(&mut client).await?, Error::TooManyInputs));
    Ok(())
}

#[tokio::test]
async fn queued_turn_counts_are_bounded_by_the_command_queue_not_the_message_queue() -> TestResult {
    let mut configured = options()?;
    configured.limits.message_queue_capacity = QueueCapacity::new(1)?;
    configured.limits.command_queue_capacity = QueueCapacity::new(2)?;
    let (mut client, mut cli) = connect(configured).await?;
    let mut accepted = result();
    accepted["queued_turn_count"] = 2.into();
    cli.send(accepted).await?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
    let mut rejected = result();
    rejected["queued_turn_count"] = 3.into();
    cli.send(rejected).await?;
    cli.end(0);
    assert!(matches!(terminal(&mut client).await?, Error::TooManyInputs));
    Ok(())
}

/// Both upstream SDKs ignore a control response for an unknown request and store any session state verbatim.
#[tokio::test]
async fn unknown_permission_suggestion_values_reach_the_callback_and_round_trip() -> TestResult {
    let suggestions = json!([
        {"type":"setMode","mode":"plan","destination":"elsewhere"},
        {"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"ls"}],"behavior":"prompt","destination":"session"},
        {"type":"futureUpdate","destination":"session","payload":{"nested":[1,null]}},
        {"type":"addDirectories","directories":["/work"],"destination":"cliArg"},
    ]);
    let observed = Arc::new(AtomicUsize::new(0));
    let seen = observed.clone();
    let mut configured = options()?;
    configured.permission_handler = Some(Arc::new(move |request: ToolPermissionRequest| {
        seen.store(request.suggestions.len(), Ordering::SeqCst);
        async move {
            PermissionDecision::Allow {
                updated_input: None,
                updated_permissions: Some(request.suggestions),
            }
        }
    }));
    let (client, mut cli) = connect(configured).await?;
    cli.send(json!({"type":"control_request","request_id":"permission","request":{
        "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"permission_suggestions":suggestions
    }}))
    .await?;
    let response = cli.read().await?;
    assert_eq!(response["response"]["subtype"], "success");
    assert_eq!(response["response"]["response"]["behavior"], "allow");
    assert_eq!(response["response"]["response"]["updatedPermissions"], suggestions);
    assert_eq!(observed.load(Ordering::SeqCst), 4);
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
async fn malformed_permission_requests_get_an_error_response_and_the_session_continues() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let mut configured = options()?;
    configured.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { PermissionDecision::allow() }
    }));
    let (mut client, mut cli) = connect(configured).await?;
    for (index, malformed) in [
        json!({"subtype":"can_use_tool","tool_name":"Bash","input":{},"permission_suggestions":[{"destination":"session"}]}),
        json!({"subtype":"can_use_tool","tool_name":"Bash","input":{},"permission_suggestions":[
            {"type":"addRules","rules":{},"behavior":"allow","destination":"session"}
        ]}),
        json!({"subtype":"can_use_tool","input":{}}),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("malformed-{index}");
        cli.send(json!({"type":"control_request","request_id":id,"request":malformed}))
            .await?;
        let response = cli.read().await?;
        assert_eq!(response["response"]["subtype"], "error");
        assert_eq!(response["response"]["request_id"], id);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    cli.send(json!({"type":"control_request","request_id":"valid","request":{
        "subtype":"can_use_tool","tool_name":"Bash","input":{},"permission_suggestions":[{"type":"future","destination":"x"}]
    }}))
    .await?;
    let response = cli.read().await?;
    assert_eq!(response["response"]["subtype"], "success");
    assert_eq!(response["response"]["response"]["behavior"], "allow");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    cli.send(result()).await?;
    loop {
        match client.next_message().await? {
            Some(Message::Result(_)) => break,
            Some(_) => {}
            None => return Err("session ended before the result".into()),
        }
    }
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
async fn s03_s04_unknown_control_responses_and_session_states_do_not_end_the_session() -> TestResult {
    let events = Arc::new(AtomicUsize::new(0));
    let _guard = tracing::subscriber::set_default(CountingSubscriber(events.clone()));
    let (mut client, mut cli) = connect(options()?).await?;
    cli.send(
        json!({"type":"control_response","response":{"subtype":"success","request_id":"unmatched","response":{}}}),
    )
    .await?;
    cli.send(json!({"type":"system","subtype":"session_state_changed","state":"future"}))
        .await?;
    cli.send(result()).await?;
    loop {
        match client.next_message().await? {
            Some(Message::Result(_)) => break,
            Some(_) => {}
            None => return Err("session ended before the result".into()),
        }
    }
    cli.disconnect(client).await?;
    assert_ne!(events.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn background_task_ledger_rejects_new_tasks_at_its_limit_but_accepts_duplicates() -> TestResult {
    let mut configured = options()?;
    configured.limits.message_queue_capacity = QueueCapacity::new(2)?;
    let (mut client, mut cli) = connect(configured).await?;
    for (kind, id) in [
        ("local_agent", "first"),
        ("local_workflow", "second"),
        ("local_agent", "first"),
    ] {
        cli.send(json!({"type":"system","subtype":"task_started","task_type":kind,"task_id":id}))
            .await?;
        assert!(matches!(client.next_message().await?, Some(Message::System(_))));
    }
    cli.send(json!({"type":"system","subtype":"task_started","task_type":"local_agent","task_id":"third"}))
        .await?;
    cli.end(0);
    assert!(matches!(terminal(&mut client).await?, Error::TooManyTasks));
    Ok(())
}

#[tokio::test]
async fn task_terminal_states_and_session_idle_complete_a_finished_run() -> TestResult {
    for status in ["completed", "failed", "stopped", "killed"] {
        let (mut client, mut cli) = connect(options()?).await?;
        client.send_prompt(Prompt::from("run with background task")).await?;
        cli.read().await?;
        client.finish_input().await?;
        cli.send(json!({"type":"system","subtype":"task_started","task_type":"local_agent","task_id":"background"}))
            .await?;
        assert!(matches!(client.next_message().await?, Some(Message::System(_))));
        cli.send(json!({"type":"system","subtype":"session_state_changed","state":"running"}))
            .await?;
        assert!(matches!(client.next_message().await?, Some(Message::System(_))));
        cli.send(json!({"type":"stream_event","uuid":"stream-one","session_id":"memory-session","event":{"type":"content_block_start"}})).await?;
        assert!(matches!(client.next_message().await?, Some(Message::StreamEvent(_))));
        cli.send(result()).await?;
        assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
        cli.send(json!({"type":"system","subtype":"task_updated","task_id":"background","patch":{"status":status}}))
            .await?;
        assert!(matches!(client.next_message().await?, Some(Message::System(_))));
        cli.send(json!({"type":"system","subtype":"session_state_changed","state":"requires_action"}))
            .await?;
        assert!(matches!(client.next_message().await?, Some(Message::System(_))));
        cli.send(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
            .await?;
        assert!(matches!(client.next_message().await?, Some(Message::System(_))));
        assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
        cli.end(0);
        assert!(client.next_message().await?.is_none());
        client.disconnect().await?;
    }
    Ok(())
}

#[tokio::test]
async fn error_results_and_queue_retractions_preserve_run_completion() -> TestResult {
    let (mut client, mut cli) = connect(options()?).await?;
    let mut first = UserInput::from(Prompt::from("first"));
    first.uuid = Some(MessageId::from("queued-first"));
    let mut second = UserInput::from(Prompt::from("second"));
    second.uuid = Some(MessageId::from("queued-second"));
    client.stream_input(futures_util::stream::iter([first, second])).await?;
    cli.read().await?;
    cli.read().await?;
    let mut response = result();
    response["queued_turn_count"] = 1.into();
    response["is_error"] = true.into();
    response["errors"] = json!(["request failed"]);
    cli.send(response).await?;
    assert!(
        matches!(client.next_message().await?, Some(Message::Result(result)) if result.status == ResultStatus::Error && result.errors.as_deref() == Some(&["request failed".to_owned()][..]))
    );
    let handle = client.handle();
    let interrupt = tokio::spawn(async move { handle.interrupt_cancel_queued().await });
    let request = cli.read().await?;
    cli.success(&request, json!({"still_queued":[],"cancelled":["queued-second"]}))
        .await?;
    interrupt.await??;
    cli.send(json!({"type":"system","subtype":"task_started","task_type":"local_agent"}))
        .await?;
    assert!(matches!(client.next_message().await?, Some(Message::System(_))));
    cli.send(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))
        .await?;
    assert!(matches!(client.next_message().await?, Some(Message::System(_))));
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.end(0);
    assert!(client.next_message().await?.is_none());
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn a_panicking_host_callback_is_a_typed_terminal_failure() -> TestResult {
    let mut configured = options()?;
    let started = Arc::new(tokio::sync::Notify::new());
    let observed = started.clone();
    configured.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        let observed = observed.clone();
        async move {
            observed.notify_one();
            std::panic::resume_unwind(Box::new("scripted host callback failure"))
        }
    }));
    let (mut client, mut cli) = connect(configured).await?;
    cli.send(json!({"type":"control_request","request_id":"host-failure","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}})).await?;
    timeout(Duration::from_secs(2), started.notified()).await?;
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.end(0);
    assert!(matches!(terminal(&mut client).await?, Error::CallbackTask(error) if error.is_panic()));
    Ok(())
}

fn configured_store(
    store: Arc<dyn SessionStore>,
    directory: &Path,
) -> Result<ClaudeAgentOptions, Box<dyn std::error::Error>> {
    let mut configured = options()?;
    let mut store_options = SessionStoreOptions::from(store);
    store_options.config_directory = Some(directory.to_path_buf());
    store_options.project_key = Some(SessionProjectKey::new("project")?);
    store_options.limits.append_timeout = Timeout::new(Duration::from_secs(10))?;
    store_options.limits.flush_timeout = Timeout::new(Duration::from_millis(100))?;
    configured.cwd = Some(directory.to_path_buf());
    configured.session_store = Some(store_options);
    Ok(configured)
}

fn mirror_frame(directory: &Path) -> Value {
    json!({"type":"transcript_mirror","filePath":directory.join("projects/project/memory-session.jsonl"),"entries":[{"type":"assistant","text":"stored"}]})
}

fn eager_mirror_frame(directory: &Path) -> Value {
    let mut frame = mirror_frame(directory);
    frame["entries"] = json!([{"type":"assistant","text":"one"},{"type":"assistant","text":"two"}]);
    frame
}

fn eager_store(
    store: Arc<dyn SessionStore>,
    directory: &Path,
) -> Result<ClaudeAgentOptions, Box<dyn std::error::Error>> {
    let mut configured = configured_store(store, directory)?;
    if let Some(store) = &mut configured.session_store {
        store.limits.max_pending_entries =
            NonZeroUsize::new(1).ok_or_else(|| io::Error::other("invalid entry limit"))?;
    }
    Ok(configured)
}

async fn control_barrier(client: &ClaudeSdkClient, cli: &mut ScriptedCli) -> TestResult {
    let handle = client.handle();
    let control = tokio::spawn(async move { handle.set_model(None).await });
    let request = cli.read().await?;
    cli.success(&request, json!({})).await?;
    control.await??;
    Ok(())
}

#[tokio::test]
async fn m13_s19_unmappable_and_oversized_mirror_frames_are_never_rejected() -> TestResult {
    let events = Arc::new(AtomicUsize::new(0));
    let _guard = tracing::subscriber::set_default(CountingSubscriber(events.clone()));
    let directory = tempfile::tempdir()?;
    let store = Arc::new(trogon_claude_agent_sdk::InMemorySessionStore::<
        trogon_claude_agent_sdk::adapters::SystemClock,
    >::default());
    let mut configured = configured_store(store.clone(), directory.path())?;
    if let Some(store) = &mut configured.session_store {
        store.limits.max_pending_entries =
            NonZeroUsize::new(1).ok_or_else(|| io::Error::other("invalid entry limit"))?;
    }
    let (client, mut cli) = connect(configured).await?;
    let mut unmappable = mirror_frame(directory.path());
    unmappable["filePath"] = directory
        .path()
        .join("outside.jsonl")
        .to_string_lossy()
        .into_owned()
        .into();
    cli.send(unmappable).await?;
    cli.send(eager_mirror_frame(directory.path())).await?;
    control_barrier(&client, &mut cli).await?;
    cli.disconnect(client).await?;
    let key = SessionKey::new(SessionProjectKey::new("project")?, "memory-session".into())?;
    let stored = store.load(&key).await?.ok_or("oversized frame was not mirrored")?;
    assert_eq!(stored.len(), 2);
    assert_ne!(events.load(Ordering::SeqCst), 0);
    Ok(())
}

struct GatedAppend {
    started: tokio::sync::Notify,
    gate: tokio::sync::Semaphore,
    fail: bool,
}

impl GatedAppend {
    fn new(fail: bool) -> Self {
        Self {
            started: tokio::sync::Notify::new(),
            gate: tokio::sync::Semaphore::new(0),
            fail,
        }
    }
}

impl SessionStore for GatedAppend {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async {
            self.started.notify_one();
            self.gate.acquire().await.map_err(SessionStoreError::backend)?.forget();
            if self.fail {
                Err(SessionStoreError::backend(io::Error::other("scripted append failure")))
            } else {
                Ok(())
            }
        })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test]
async fn shutdown_collects_each_queued_mirror_fence_and_retains_append_failures() -> TestResult {
    for fail in [false, true] {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(GatedAppend::new(fail));
        let mut configured = eager_store(store.clone(), directory.path())?;
        if let Some(store) = &mut configured.session_store {
            // A failing append is retried twice with backoff before it is reported.
            store.limits.flush_timeout = Timeout::new(Duration::from_secs(5))?;
        }
        let (client, mut cli) = connect(configured).await?;
        client.send_prompt(Prompt::from("mirror run")).await?;
        cli.read().await?;
        cli.send(eager_mirror_frame(directory.path())).await?;
        timeout(Duration::from_secs(2), store.started.notified()).await?;
        cli.send(result()).await?;
        cli.send(result()).await?;
        control_barrier(&client, &mut cli).await?;
        let closing = tokio::spawn(client.disconnect());
        assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
        cli.end(0);
        store.gate.add_permits(if fail { 3 } else { 1 });
        let outcome = closing.await?;
        outcome?;
    }
    Ok(())
}

fn flush_timeout_leaves(error: &Error) -> Option<usize> {
    match error {
        Error::Session(SessionError::MirrorFlushTimeout) => Some(1),
        Error::Cleanup { primary, cleanup } => Some(flush_timeout_leaves(primary)? + flush_timeout_leaves(cleanup)?),
        _ => None,
    }
}

#[tokio::test]
async fn shutdown_retains_errors_from_active_deferred_and_final_mirror_flushes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Arc::new(GatedAppend::new(false));
    let (client, mut cli) = connect(eager_store(store.clone(), directory.path())?).await?;
    client.send_prompt(Prompt::from("mirror run")).await?;
    cli.read().await?;
    cli.send(eager_mirror_frame(directory.path())).await?;
    timeout(Duration::from_secs(2), store.started.notified()).await?;
    cli.send(result()).await?;
    cli.send(result()).await?;
    control_barrier(&client, &mut cli).await?;
    let closing = tokio::spawn(client.disconnect());
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.end(0);
    let outcome = closing.await?;
    assert!(matches!(outcome, Err(error) if flush_timeout_leaves(&error) == Some(3)));
    Ok(())
}

#[tokio::test]
async fn mirror_fence_does_not_make_the_deferred_message_queue_unbounded() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Arc::new(GatedAppend::new(false));
    let mut configured = eager_store(store.clone(), directory.path())?;
    configured.limits.message_queue_capacity = QueueCapacity::new(1)?;
    let (mut client, mut cli) = connect(configured).await?;
    client.send_prompt(Prompt::from("bounded mirror run")).await?;
    cli.read().await?;
    cli.send(eager_mirror_frame(directory.path())).await?;
    timeout(Duration::from_secs(2), store.started.notified()).await?;
    cli.send(result()).await?;
    cli.send(json!({"type":"system","subtype":"after_result"})).await?;
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.end(0);
    store.gate.add_permits(1);
    assert!(matches!(terminal(&mut client).await?, Error::MessageQueueFull));
    Ok(())
}

#[tokio::test]
async fn owner_disposal_interrupts_a_backpressured_write() -> TestResult {
    let mut configured = options()?;
    configured.limits.max_frame_bytes = NonZeroUsize::new(16 * 1024 * 1024).ok_or("invalid limit")?;
    let (client, cli) = connect(configured).await?;
    let ScriptedCli {
        lines,
        stdout,
        stderr,
        observer,
    } = cli;
    let mut input = lines.into_inner();
    let handle = client.handle();
    let prompt = Prompt::from("x".repeat(8 * 1024 * 1024));
    let sending = tokio::spawn(async move { handle.send_prompt(prompt).await });
    let mut first = [0_u8; 1];
    timeout(Duration::from_secs(2), input.read_exact(&mut first)).await??;
    drop(client);
    let mut remainder = Vec::new();
    timeout(Duration::from_secs(2), input.read_to_end(&mut remainder)).await??;
    observer.complete(0);
    drop(stdout);
    drop(stderr);
    assert!(matches!(sending.await?, Err(Error::DriverStopped)));
    observer.wait_for(ScriptedProcessEvent::StdinClosed).await?;
    Ok(())
}

#[tokio::test]
async fn a_declared_dialog_without_a_handler_does_not_emit_a_response() -> TestResult {
    let mut configured = options()?;
    configured.supported_dialog_kinds = vec![DialogKind::from("fixture")];
    let (client, mut cli) = connect(configured).await?;
    cli.send(json!({"type":"control_request","request_id":"ignored","request":{"subtype":"request_user_dialog","dialog_kind":"fixture","payload":{}}})).await?;
    let handle = client.handle();
    let control = tokio::spawn(async move { handle.set_model(None).await });
    let request = cli.read().await?;
    assert_eq!(request["request"]["subtype"], "set_model");
    cli.success(&request, json!({})).await?;
    control.await??;
    cli.disconnect(client).await?;
    Ok(())
}

#[tokio::test]
async fn python_style_reports_a_control_request_after_the_run_settled_as_a_terminated_process() -> TestResult {
    let mut python = options()?;
    python.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    let (mut client, mut cli) = connect(python).await?;
    client.send_prompt(Prompt::from("settle")).await?;
    client.finish_input().await?;
    cli.read().await?;
    cli.send(result()).await?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    let handle = client.handle();
    assert!(matches!(
        handle.set_model(None).await,
        Err(Error::NotReadyForWriting | Error::WriteToTerminatedProcess { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_control_request_after_the_run_settled_fails_only_that_request() -> TestResult {
    let (mut client, mut cli) = connect(options()?).await?;
    client.send_prompt(Prompt::from("settle")).await?;
    client.finish_input().await?;
    cli.read().await?;
    cli.send(result()).await?;
    assert!(matches!(client.next_message().await?, Some(Message::Result(_))));
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    let handle = client.handle();
    assert!(matches!(handle.set_model(None).await, Err(Error::NotReadyForWriting)));
    assert!(matches!(
        handle.set_mcp_servers(BTreeMap::new(), BTreeMap::new()).await,
        Err(Error::NotReadyForWriting)
    ));
    assert!(matches!(
        handle.send_prompt(Prompt::from("late")).await,
        Err(Error::NotReadyForWriting | Error::InputClosed)
    ));
    cli.end(0);
    assert!(client.next_message().await?.is_none());
    client.disconnect().await?;
    Ok(())
}

#[tokio::test]
async fn a_turn_keeps_stdin_open_until_an_in_flight_callback_answers() -> TestResult {
    let started = Arc::new(tokio::sync::Notify::new());
    let observed = started.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let released = release.clone();
    let mut configured = options()?;
    configured.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        observed.notify_one();
        let released = released.clone();
        async move {
            released.notified().await;
            PermissionDecision::allow()
        }
    }));
    let (process, harness) = ScriptedProcessSpawner::new();
    let mut cli = ScriptedCli::from(harness);
    let querying = tokio::spawn(query_turn_with_adapters(
        Prompt::from("settle"),
        configured,
        adapters(process),
    ));
    let request = cli.read().await?;
    cli.success(&request, json!({})).await?;
    let mut query = querying.await??;
    cli.read().await?;
    cli.send(json!({"type":"control_request","request_id":"late","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}})).await?;
    timeout(Duration::from_secs(2), started.notified()).await?;
    cli.send(result()).await?;
    assert!(matches!(query.next_message().await?, Some(Message::Result(_))));
    release.notify_one();
    let response = cli.read().await?;
    assert_eq!(response["response"]["request_id"], "late");
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.end(0);
    assert!(query.next_message().await?.is_none());
    query.close().await?;
    Ok(())
}

#[tokio::test]
async fn a_callback_answered_after_the_run_settled_is_dropped_without_stopping_the_driver() -> TestResult {
    let dropped = Arc::new(tokio::sync::Notify::new());
    let _guard = tracing::subscriber::set_default(EventWatcher {
        message: "dropping a control response written after stdin closed",
        seen: dropped.clone(),
    });
    let started = Arc::new(tokio::sync::Notify::new());
    let observed = started.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let released = release.clone();
    let mut configured = options()?;
    configured.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        observed.notify_one();
        let released = released.clone();
        async move {
            released.notified().await;
            PermissionDecision::allow()
        }
    }));
    let (process, harness) = ScriptedProcessSpawner::new();
    let mut cli = ScriptedCli::from(harness);
    let querying = tokio::spawn(query_turn_with_adapters(
        Prompt::from("settle"),
        configured,
        adapters(process),
    ));
    let request = cli.read().await?;
    cli.success(&request, json!({})).await?;
    let mut query = querying.await??;
    cli.read().await?;
    cli.send(result()).await?;
    assert!(matches!(query.next_message().await?, Some(Message::Result(_))));
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.send(json!({"type":"control_request","request_id":"late","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}})).await?;
    timeout(Duration::from_secs(2), started.notified()).await?;
    release.notify_one();
    // The driver must log the dropped answer before the child exits, or it can see the exit first.
    timeout(Duration::from_secs(2), dropped.notified()).await?;
    cli.end(0);
    assert!(query.next_message().await?.is_none());
    query.close().await?;
    Ok(())
}

/// Enables every callsite and wakes a test when an event carries `message`.
struct EventWatcher {
    message: &'static str,
    seen: Arc<tokio::sync::Notify>,
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for EventWatcher {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if visitor.0 == self.message {
            self.seen.notify_one();
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Enables every callsite so the fields of a log event are evaluated, and counts the events.
struct CountingSubscriber(Arc<AtomicUsize>);

impl tracing::Subscriber for CountingSubscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn a_second_input_beyond_the_queue_capacity_is_rejected_without_ending_the_session() -> TestResult {
    let mut configured = options()?;
    configured.limits.command_queue_capacity = QueueCapacity::new(1)?;
    let (client, mut cli) = connect(configured).await?;
    client.send_prompt(Prompt::from("first")).await?;
    assert_eq!(cli.read().await?["message"]["content"], "first");
    assert!(matches!(
        client.send_prompt(Prompt::from("second")).await,
        Err(Error::TooManyInputs)
    ));
    cli.disconnect(client).await?;
    Ok(())
}

#[derive(Debug)]
struct PanickingAppend {
    started: tokio::sync::Notify,
}

impl SessionStore for PanickingAppend {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        self.started.notify_one();
        std::panic::resume_unwind(Box::new("scripted append panic"))
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

fn is_mirror_stopped(error: &Error) -> bool {
    match error {
        Error::Session(SessionError::MirrorStopped) => true,
        Error::Cleanup { primary, cleanup } => is_mirror_stopped(primary) || is_mirror_stopped(cleanup),
        _ => false,
    }
}

#[tokio::test]
async fn a_mirror_frame_after_the_mirror_worker_stopped_is_a_terminal_session_error() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Arc::new(PanickingAppend {
        started: tokio::sync::Notify::new(),
    });
    let (mut client, mut cli) = connect(eager_store(store.clone(), directory.path())?).await?;
    cli.send(eager_mirror_frame(directory.path())).await?;
    timeout(Duration::from_secs(2), store.started.notified()).await?;
    control_barrier(&client, &mut cli).await?;
    cli.send(mirror_frame(directory.path())).await?;
    cli.end(0);
    assert!(is_mirror_stopped(&terminal(&mut client).await?));
    Ok(())
}

#[tokio::test]
async fn a_mirror_failure_that_cannot_be_published_is_logged_and_does_not_fail_shutdown() -> TestResult {
    let events = Arc::new(AtomicUsize::new(0));
    let _guard = tracing::subscriber::set_default(CountingSubscriber(events.clone()));
    let directory = tempfile::tempdir()?;
    let store = Arc::new(GatedAppend::new(true));
    let mut configured = eager_store(store.clone(), directory.path())?;
    configured.limits.message_queue_capacity = QueueCapacity::new(1)?;
    if let Some(store) = &mut configured.session_store {
        store.limits.flush_timeout = Timeout::new(Duration::from_secs(5))?;
    }
    let (client, mut cli) = connect(configured).await?;
    cli.send(json!({"type":"system","subtype":"fills_the_queue"})).await?;
    cli.send(eager_mirror_frame(directory.path())).await?;
    timeout(Duration::from_secs(2), store.started.notified()).await?;
    control_barrier(&client, &mut cli).await?;
    store.gate.add_permits(3);
    let closing = tokio::spawn(client.disconnect());
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.end(0);
    closing.await??;
    assert_ne!(events.load(Ordering::SeqCst), 0);
    Ok(())
}

/// Hands the runtime a stderr pipe that outlives the CLI, as a grandchild that inherited it would.
struct HeldStderr(ScriptedProcessSpawner);

impl SpawnProcess for HeldStderr {
    type Error = io::Error;

    fn spawn(&self, command: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
        let mut child = self.0.spawn(command)?;
        let mut holder = tokio::process::Command::new("sleep");
        holder
            .arg("3")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        child.stderr = TokioProcessSpawner.spawn(&mut holder)?.stderr.take();
        Ok(child)
    }
}

async fn connect_holding_stderr() -> Result<(ClaudeSdkClient, ScriptedCli), Box<dyn std::error::Error>> {
    // Python style: TypeScript skips an undecodable line, so only Python ends the session on it.
    let mut configured = options()?;
    configured.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    configured.limits.shutdown_timeout = Timeout::new(Duration::from_millis(300))?;
    let (process, harness) = ScriptedProcessSpawner::new();
    let mut cli = ScriptedCli::from(harness);
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_adapters(
        configured,
        adapters(HeldStderr(process)),
    ));
    let request = cli.read().await?;
    cli.success(&request, json!({"hooks_applied":true,"hook_on_failure_supported":true}))
        .await?;
    Ok((connecting.await??, cli))
}

#[tokio::test]
async fn a_stderr_pipe_that_stays_open_after_the_cli_exits_fails_a_clean_shutdown() -> TestResult {
    let (client, mut cli) = connect_holding_stderr().await?;
    let closing = tokio::spawn(client.disconnect());
    assert!(timeout(Duration::from_secs(2), cli.lines.next_line()).await??.is_none());
    cli.end(0);
    assert!(matches!(closing.await?, Err(Error::StderrDrainTimeout)));
    Ok(())
}

#[tokio::test]
async fn a_stderr_pipe_that_stays_open_is_reported_beside_the_error_that_ended_the_session() -> TestResult {
    let (mut client, mut cli) = connect_holding_stderr().await?;
    cli.stdout.write_all(b"{\"type\":\n").await?;
    cli.end(0);
    assert!(matches!(
        terminal(&mut client).await?,
        Error::Cleanup { primary, cleanup }
            if matches!(*primary, Error::JsonDecode(_)) && matches!(*cleanup, Error::StderrDrainTimeout)
    ));
    Ok(())
}

/// Whether the runtime saw the CLI exit before the prompt arrived, which races with the prompt.
async fn startup_fails_before_a_healthy_shutdown() -> Result<bool, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let mut cli = ScriptedCli::from(harness);
    let querying = tokio::spawn(query_with_adapters(
        Prompt::from("prompt"),
        options()?,
        adapters(process),
    ));
    let request = cli.read().await?;
    cli.success(&request, json!({"hooks_applied":true,"hook_on_failure_supported":true}))
        .await?;
    cli.observer.complete(0);
    tokio::task::yield_now().await;
    // Nothing runs while this thread waits, so the response and the exit of the CLI usually reach
    // the runtime together and the exit is seen before the prompt is sent.
    std::thread::sleep(Duration::from_millis(100));
    drop(cli.stdout);
    drop(cli.stderr);
    Ok(matches!(querying.await?, Err(Error::NotReadyForWriting)))
}

#[tokio::test]
async fn a_startup_failure_on_a_healthy_shutdown_reports_only_the_startup_error() -> TestResult {
    for _ in 0..40 {
        if startup_fails_before_a_healthy_shutdown().await? {
            return Ok(());
        }
    }
    Err("the CLI exit was never observed before the prompt".into())
}

#[tokio::test]
async fn a_startup_failure_is_reported_beside_the_error_that_stopped_the_session() -> TestResult {
    let (process, harness) = ScriptedProcessSpawner::new();
    let mut cli = ScriptedCli::from(harness);
    // Python style: TypeScript skips an undecodable line, so only Python fails on it.
    let mut python = options()?;
    python.argv_style = trogon_claude_agent_sdk::ArgvStyle::Python;
    let querying = tokio::spawn(query_with_adapters(Prompt::from("prompt"), python, adapters(process)));
    let request = cli.read().await?;
    let response = json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{}}});
    cli.stdout
        .write_all(format!("{response}\n{{\"type\":\n").as_bytes())
        .await?;
    cli.end(0);
    assert!(matches!(
        querying.await?,
        Err(Error::Cleanup { primary, cleanup })
            if matches!(*primary, Error::WriteToTerminatedProcess { .. }) && matches!(*cleanup, Error::JsonDecode(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_mirror_frame_is_ignored_when_no_session_store_is_configured() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (client, mut cli) = connect(options()?).await?;
    cli.send(mirror_frame(directory.path())).await?;
    control_barrier(&client, &mut cli).await?;
    cli.disconnect(client).await?;
    Ok(())
}
