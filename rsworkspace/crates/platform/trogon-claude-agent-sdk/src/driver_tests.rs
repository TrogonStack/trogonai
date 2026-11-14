use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use tokio::sync::mpsc;

use super::*;
use crate::{ToolPermissionRequest, ToolSet, environment_plan::Inherited, process_transport::command};

/// A transport the test scripts: documents the driver writes arrive on `written`, and objects sent on
/// `incoming` are what the driver reads.
struct ScriptedTransport {
    written: mpsc::UnboundedSender<String>,
    incoming: mpsc::UnboundedReceiver<JsonObject>,
    ready: bool,
}

impl Transport for ScriptedTransport {
    async fn connect(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn write(&mut self, data: &str) -> Result<(), Error> {
        self.written.send(data.to_owned()).map_err(|_| Error::DriverStopped)
    }

    async fn read_message(&mut self) -> Result<Option<JsonObject>, Error> {
        Ok(self.incoming.recv().await)
    }

    async fn close(&mut self) -> Result<(), Error> {
        self.ready = false;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    async fn end_input(&mut self) -> Result<(), Error> {
        self.ready = false;
        Ok(())
    }
}

/// The far end of the scripted transport a driver under test owns.
struct Harness {
    written: mpsc::UnboundedReceiver<String>,
    _incoming: mpsc::UnboundedSender<JsonObject>,
}

struct DriverFixture {
    driver: Driver<ScriptedTransport, crate::adapters::NativeFileSystem>,
    harness: Harness,
    commands: crate::channel::Sender<Command>,
    messages: crate::channel::Receiver<Message>,
    shutdown: oneshot::Sender<()>,
}

impl DriverFixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_handler(None)
    }

    fn with_handler(handler: Option<Arc<dyn ToolPermissionHandler>>) -> Result<Self, Box<dyn std::error::Error>> {
        let (written_tx, written) = mpsc::unbounded_channel();
        let (incoming_tx, incoming) = mpsc::unbounded_channel();
        let capacity = Some(crate::QueueCapacity::new(4)?);
        let (commands, command_rx) = crate::channel::channel(capacity);
        let (message_tx, messages) = crate::channel::channel(capacity);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let driver = Driver {
            transport: ScriptedTransport {
                written: written_tx,
                incoming,
                ready: true,
            },
            input_ended: false,
            commands: command_rx,
            messages: message_tx,
            overflow: VecDeque::new(),
            shutdown: shutdown_rx,
            shutdown_seen: false,
            limits: Limits::default(),
            handler,
            message_decoding: crate::MessageDecoding::TypeScript,
            js_runtime: crate::JsRuntime::Node,
            pending: HashMap::new(),
            callbacks: JoinSet::new(),
            callback_handles: HashMap::new(),
            callback_signals: HashMap::new(),
            detached_callbacks: HashMap::new(),
            cancelled_callbacks: HashSet::new(),
            hook_callbacks: HashMap::new(),
            sdk_mcp_servers: McpSessions::new(),
            mcp: McpRouting::new(),
            capture_manifests: false,
            initialize_request: crate::wire::runtime_payload::InitializeRequest::default()
                .into_payload(crate::ArgvStyle::TypeScript)
                .to_value()?,
            initialize_expectations: InitializeExpectations::default(),
            initialize_timeout: None,
            version_check: None,
            latest_commands: LatestCommands::default(),
            verbatim_prompts: false,
            warn_unsupported_verbatim: false,
            elicitation_handler: None,
            user_dialog_handler: None,
            oauth_token_handler: None,
            host_auth_token_handler: None,
            session: None,
            mirror_flush: None,
            deferred_messages: VecDeque::new(),
            next_id: 0,
            wire_ids: None,
            state: State::Idle,
            run_state: RunState::default(),
            argv_style: crate::ArgvStyle::TypeScript,
            last_error_result: None,
        };
        Ok(Self {
            driver,
            harness: Harness {
                written,
                _incoming: incoming_tx,
            },
            commands,
            messages,
            shutdown,
        })
    }

    async fn finish(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.driver.callbacks.abort_all();
        self.driver.end_input().await;
        self.driver.transport.close().await?;
        drop(self.driver);
        assert!(
            self.harness.written.recv().await.is_none(),
            "a defensive operation wrote to the transport"
        );
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn stale_permission_completion_cannot_remove_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = move |_: ToolPermissionRequest| {
        let call = calls.fetch_add(1, Ordering::SeqCst);
        async move {
            if call == 0 {
                PermissionDecision::allow()
            } else {
                pending::<PermissionDecision>().await
            }
        }
    };
    let mut fixture = DriverFixture::with_handler(Some(Arc::new(handler)))?;
    let id = ControlRequestId::from("reused-id");
    let permission = || -> Result<IncomingControl, ProtocolError> {
        match wire::parse_frame(br#"{"type":"control_request","request_id":"reused-id","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#)? {
            Frame::ControlRequest { request, .. } => Ok(request),
            _ => Err(ProtocolError::DuplicateControlRequest),
        }
    };
    fixture
        .driver
        .frame(Frame::ControlRequest {
            id: id.clone(),
            request: permission()?,
        })
        .await?;
    timeout(Duration::from_secs(1), async {
        while !fixture
            .driver
            .callback_handles
            .get(&id)
            .is_some_and(AbortHandle::is_finished)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    fixture.driver.frame(Frame::Cancel { id: id.clone() }).await?;
    fixture
        .driver
        .frame(Frame::ControlRequest {
            id: id.clone(),
            request: permission()?,
        })
        .await?;
    let started = Instant::now();
    let outcome = timeout(Duration::from_millis(100), fixture.driver.event_loop()).await;
    let elapsed = started.elapsed();
    let replacement_retained = fixture.driver.callback_handles.contains_key(&id);
    assert!(
        replacement_retained,
        "a stale allow completion removed the replacement callback"
    );
    assert!(outcome.is_err(), "the event loop stopped");
    assert!(elapsed >= Duration::from_millis(100));
    assert!(elapsed <= Duration::from_millis(101));
    // TypeScript still writes what a cancelled handler returns.
    let stale = fixture.harness.written.try_recv()?;
    assert!(
        stale.contains(r#""request_id":"reused-id""#) && stale.contains(r#""behavior":"allow""#),
        "{stale}"
    );
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn initialization_owns_admission_until_the_handshake_completes() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.state = State::Initializing;
    let (response, receiver) = oneshot::channel();
    fixture
        .driver
        .command(Command::Prompt {
            input: crate::Prompt::from("before handshake").into(),
            response,
        })
        .await?;
    assert!(matches!(receiver.await?, Err(Error::NotInitialized)));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn request_counter_exhaustion_never_reuses_an_id_or_writes_a_frame() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.limits.request_id_exhaustion = crate::RequestIdExhaustion::Fail;
    fixture.driver.next_id = u64::MAX;
    let (response, _receiver) = oneshot::channel();
    let outcome = fixture
        .driver
        .start_control(
            ControlResponder::Control {
                operation: ControlOperation::SetModel,
                response,
            },
            RequestPayload::SetModel { model: None }.to_value()?,
        )
        .await;
    assert!(matches!(outcome, Err(Error::RequestIdExhausted)));
    assert_eq!(fixture.driver.next_id, u64::MAX);
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_already_requested_wins_before_more_input_is_read() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let (replacement, _receiver) = oneshot::channel();
    let shutdown = std::mem::replace(&mut fixture.shutdown, replacement);
    shutdown.send(()).map_err(|_| Error::DriverStopped)?;
    fixture.driver.event_loop().await?;
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn closing_the_command_port_ends_the_loop_without_another_cli_frame() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let (replacement, _receiver) = crate::channel::channel(None);
    let commands = std::mem::replace(&mut fixture.commands, replacement);
    drop(commands);
    fixture.driver.event_loop().await?;
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn an_expired_control_is_rejected_before_another_loop_iteration() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let (response, receiver) = oneshot::channel();
    fixture.driver.pending.insert(
        ControlRequestId::from("expired"),
        PendingControl {
            response: ControlResponder::Control {
                operation: ControlOperation::SetModel,
                response,
            },
            deadline: Some(Instant::now()),
        },
    );
    assert!(matches!(
        fixture.driver.event_loop().await,
        Err(Error::ControlTimeout {
            operation: ControlOperation::SetModel
        })
    ));
    assert!(matches!(
        receiver.await?,
        Err(Error::ControlTimeout {
            operation: ControlOperation::SetModel
        })
    ));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn an_abandoned_message_consumer_stops_publication() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.messages.close();
    let message = Message::System(crate::SystemMessage {
        subtype: "receipt".into(),
        session_id: None,
        data: JsonObject::new(),
        event: crate::SystemEvent::Other,
        raw: JsonObject::new(),
    });
    assert!(matches!(fixture.driver.publish(message), Err(Error::DriverStopped)));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn a_stale_panicked_callback_cannot_terminate_the_current_session() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let id = ControlRequestId::from("stale-panic");
    fixture.driver.spawn_callback(
        id.clone(),
        crate::CancellationToken::new(),
        Some(Duration::from_secs(1)),
        wire::callback_error_response(&id, "fallback").map(Some),
        async { std::panic::resume_unwind(Box::new("scripted stale callback failure")) },
    );
    timeout(Duration::from_secs(1), async {
        while !fixture
            .driver
            .callback_handles
            .get(&id)
            .is_some_and(AbortHandle::is_finished)
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    fixture.driver.frame(Frame::Cancel { id }).await?;
    assert!(
        timeout(Duration::from_millis(1), fixture.driver.event_loop())
            .await
            .is_err()
    );
    assert!(fixture.driver.callback_handles.is_empty());
    fixture.finish().await
}

struct FailedAppend;
impl crate::SessionStore for FailedAppend {
    fn append<'a>(
        &'a self,
        _: &'a crate::SessionKey,
        _: &'a [crate::SessionEntry],
    ) -> crate::SessionStoreFuture<'a, ()> {
        Box::pin(async {
            Err(crate::SessionStoreError::backend(std::io::Error::other(
                "scripted append failure",
            )))
        })
    }
    fn load<'a>(&'a self, _: &'a crate::SessionKey) -> crate::SessionStoreFuture<'a, Option<Vec<crate::SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test(start_paused = true)]
async fn eof_mirror_drain_publishes_failures_before_deferred_messages() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let projects = std::path::PathBuf::from("/memory-config/projects");
    let options = crate::SessionStoreOptions::from(Arc::new(FailedAppend));
    let mirror = crate::TranscriptMirror::new(&options, projects.clone());
    mirror.enqueue(
        &projects.join("project/session.jsonl"),
        vec![crate::SessionEntry::new(serde_json::from_str(
            r#"{"type":"assistant"}"#,
        )?)],
    )?;
    let flush = mirror.begin_flush()?;
    fixture.driver.mirror_flush = Some(Box::pin(async move { flush.wait().await }));
    fixture.driver.deferred_messages.push_back((
        Message::System(crate::SystemMessage {
            subtype: "after_fence".into(),
            session_id: None,
            data: JsonObject::new(),
            event: crate::SystemEvent::Other,
            raw: JsonObject::new(),
        }),
        None,
    ));
    fixture.driver.finish_mirror().await?;
    assert!(
        matches!(fixture.messages.recv().await, Some(Message::System(message)) if message.subtype == "mirror_error")
    );
    assert!(
        matches!(fixture.messages.recv().await, Some(Message::System(message)) if message.subtype == "after_fence")
    );
    fixture.finish().await
}

#[test]
fn tool_lists_reach_the_cli_verbatim() -> Result<(), Error> {
    let options = ClaudeAgentOptions {
        allowed_tools: vec![crate::ToolRule::from("Read,Write")],
        disallowed_tools: vec![crate::ToolRule::from("")],
        tools: ToolSet::Only(vec![crate::BuiltinToolName::from("Read Bash")]),
        ..ClaudeAgentOptions::default()
    };
    let command = command(&options, std::path::Path::new("claude"), &Inherited::new(), None)?;
    let arguments: Vec<_> = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    for expected in ["--allowedTools=Read,Write", "--disallowedTools=", "--tools=Read Bash"] {
        assert!(arguments.iter().any(|argument| argument == expected));
    }
    Ok(())
}

fn plugin_options(delivery: crate::PluginDelivery) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        plugin_delivery: delivery,
        plugins: vec![
            crate::LocalPlugin::new("/p/one"),
            crate::LocalPlugin {
                path: "/p/two".into(),
                skip_mcp_discovery: Some(true),
            },
        ],
        ..ClaudeAgentOptions::default()
    }
}

#[test]
fn initialize_delivery_sends_the_plugins_in_the_initialize_request() -> Result<(), Error> {
    let (request, _, _) = initialize_config(&plugin_options(crate::PluginDelivery::Initialize))?;
    assert_eq!(
        request["plugins"],
        serde_json::json!([
            {"type": "local", "path": "/p/one"},
            {"type": "local", "path": "/p/two", "skipMcpDiscovery": true}
        ])
    );
    Ok(())
}

#[test]
fn argv_delivery_leaves_the_initialize_request_without_plugins() -> Result<(), Error> {
    let (request, _, _) = initialize_config(&plugin_options(crate::PluginDelivery::Argv))?;
    assert!(request.get("plugins").is_none());
    Ok(())
}

fn derived_label_in(options: &ClaudeAgentOptions, inherited: &Inherited, cli: &std::path::Path) -> Option<String> {
    let command = command(options, cli, inherited, None).ok()?;
    command
        .as_std()
        .get_envs()
        .find(|(key, _)| *key == "CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP")
        .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
}

#[test]
fn a_derived_client_app_reaches_the_overlay_environment_unless_a_label_exists() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    std::fs::create_dir(root.path().join("_bundled"))?;
    let cli = root
        .path()
        .join("_bundled")
        .join(if cfg!(windows) { "claude.exe" } else { "claude" });
    std::fs::write(&cli, "")?;
    std::fs::write(cli.with_file_name("claude.version"), "999.0.0")?;
    let derived = ClaudeAgentOptions {
        environment_mode: crate::EnvironmentMode::Overlay,
        client_app_derivation: crate::ClientAppDerivation::Bundled(crate::BundledCli {
            package_root: root.path().to_path_buf(),
            pinned_version: "999.0.0".to_owned(),
        }),
        ..ClaudeAgentOptions::default()
    };
    let label = derived_label_in(&derived, &Inherited::new(), &cli);
    assert_eq!(label, Some(format!("trogon-claude-agent-sdk/{}", crate::SDK_VERSION)));
    let explicit = ClaudeAgentOptions {
        client_app: Some(String::new()),
        ..derived.clone()
    };
    assert_eq!(derived_label_in(&explicit, &Inherited::new(), &cli), None);
    let inherited = Inherited::from([("CLAUDE_AGENT_SDK_CLIENT_APP".into(), "outer".into())]);
    assert_eq!(derived_label_in(&derived, &inherited, &cli), None);
    let inheriting = ClaudeAgentOptions {
        environment_mode: crate::EnvironmentMode::Inherit,
        ..derived
    };
    assert_eq!(derived_label_in(&inheriting, &Inherited::new(), &cli), None);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_request_counter_wraps_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.argv_style = crate::ArgvStyle::Python;
    fixture.driver.next_id = u64::MAX;
    assert!(fixture.driver.next_request_id()?.as_str().starts_with("req_0_"));
    assert!(fixture.driver.next_request_id()?.as_str().starts_with("req_1_"));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn a_typescript_initialize_expiry_is_initialize_not_confirmed() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let (response, receiver) = oneshot::channel();
    fixture.driver.pending.insert(
        ControlRequestId::from("initialize"),
        PendingControl {
            response: ControlResponder::Initialize { first: true, response },
            deadline: Some(Instant::now()),
        },
    );
    assert!(matches!(
        fixture.driver.event_loop().await,
        Err(Error::InitializeNotConfirmed)
    ));
    assert!(matches!(receiver.await?, Err(Error::InitializeNotConfirmed)));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn an_initialize_armed_only_by_managed_settings_expires_with_their_message()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.initialize_expectations.managed_settings = true;
    let (response, receiver) = oneshot::channel();
    fixture.driver.pending.insert(
        ControlRequestId::from("initialize"),
        PendingControl {
            response: ControlResponder::Initialize { first: true, response },
            deadline: Some(Instant::now()),
        },
    );
    assert!(matches!(
        fixture.driver.event_loop().await,
        Err(Error::ManagedSettingsNotConfirmed)
    ));
    assert!(matches!(receiver.await?, Err(Error::ManagedSettingsNotConfirmed)));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn controls_have_no_deadline_unless_one_is_configured() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    assert!(fixture.driver.control_deadline(ControlOperation::SetModel).is_none());
    assert!(fixture.driver.control_deadline(ControlOperation::Initialize).is_none());
    fixture.driver.limits.control_timeout = Some(crate::Timeout::default());
    fixture.driver.initialize_timeout = Some(Duration::from_secs(1));
    let now = Instant::now();
    assert_eq!(
        fixture.driver.control_deadline(ControlOperation::SetModel),
        Some(now + Duration::from_secs(60))
    );
    assert_eq!(
        fixture.driver.control_deadline(ControlOperation::Initialize),
        Some(now + Duration::from_secs(1))
    );
    fixture.driver.limits.control_timeout = Some(crate::Timeout::new(Duration::MAX));
    assert!(fixture.driver.control_deadline(ControlOperation::SetModel).is_none());
    fixture.finish().await
}

fn receipt(subtype: &str) -> Message {
    Message::System(crate::SystemMessage {
        subtype: subtype.into(),
        session_id: None,
        data: JsonObject::new(),
        event: crate::SystemEvent::Other,
        raw: JsonObject::new(),
    })
}

#[tokio::test(start_paused = true)]
async fn a_full_buffer_holds_messages_back_unless_set_to_fail() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.limits.message_buffer = crate::MessageBuffer::Backpressure(crate::QueueCapacity::new(4)?);
    for index in 0..6 {
        fixture.driver.publish(receipt(&index.to_string()))?;
    }
    assert_eq!(fixture.driver.overflow.len(), 2);
    for expected in ["0", "1"] {
        assert!(matches!(fixture.messages.recv().await, Some(Message::System(message)) if message.subtype == expected));
    }
    fixture.driver.drain_overflow()?;
    assert!(fixture.driver.overflow.is_empty());
    fixture.driver.limits.message_buffer = crate::MessageBuffer::FailWhenFull(crate::QueueCapacity::new(4)?);
    assert!(matches!(
        fixture.driver.publish(receipt("full")),
        Err(Error::MessageQueueFull)
    ));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn backpressure_stops_reading_and_overflow_reaches_the_caller() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.limits.message_buffer = crate::MessageBuffer::Backpressure(crate::QueueCapacity::new(4)?);
    fixture
        .driver
        .deferred_messages
        .extend((0..4).map(|index| (receipt(&index.to_string()), None)));
    assert!(fixture.driver.deferred_full());
    fixture.driver.deferred_messages.clear();
    for index in 0..5 {
        fixture.driver.publish(receipt(&index.to_string()))?;
    }
    let reader = async {
        let mut seen = Vec::new();
        while let Some(Message::System(message)) = fixture.messages.recv().await {
            seen.push(message.subtype);
            if seen.len() == 5 {
                break;
            }
        }
        seen
    };
    let (_, seen) = tokio::join!(fixture.driver.flush_overflow(), reader);
    assert_eq!(seen, ["0", "1", "2", "3", "4"]);
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn a_duplicate_in_flight_request_is_skipped_or_run_by_policy() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let id = ControlRequestId::from("twice");
    fixture.driver.spawn_callback(
        id.clone(),
        crate::CancellationToken::new(),
        None,
        wire::callback_error_response(&id, "fallback").map(Some),
        pending(),
    );
    assert!(!fixture.driver.admit_callback(&id).await?);
    assert_eq!(fixture.driver.running_callbacks(), 1);
    fixture.driver.limits.duplicate_control_requests = crate::DuplicateControlRequests::Fail;
    assert!(matches!(
        fixture.driver.admit_callback(&id).await,
        Err(Error::Protocol(ProtocolError::DuplicateControlRequest))
    ));
    fixture.driver.limits.duplicate_control_requests = crate::DuplicateControlRequests::RunBoth;
    assert!(fixture.driver.admit_callback(&id).await?);
    assert_eq!(fixture.driver.detached_callbacks.len(), 1);
    assert!(!fixture.driver.callback_handles.contains_key(&id));
    fixture.driver.limits.max_incoming_callbacks = Some(crate::QueueCapacity::new(1)?);
    let other = ControlRequestId::from("other");
    assert!(!fixture.driver.admit_callback(&other).await?);
    let written = fixture.harness.written.recv().await.unwrap_or_default();
    assert!(written.contains("Too many outstanding SDK callbacks"));
    fixture.finish().await
}

#[test]
fn python_initialize_timeout_reads_the_stream_close_variable() -> Result<(), Error> {
    let parse = |value: &str| python_initialize_timeout(Some(value.into()));
    assert_eq!(python_initialize_timeout(None)?, Duration::from_secs(60));
    assert_eq!(parse("90000")?, Duration::from_secs(90));
    assert_eq!(parse(" +120_500\n")?, Duration::from_millis(120_500));
    assert_eq!(parse("1000")?, Duration::from_secs(60));
    assert_eq!(parse("-5")?, Duration::from_secs(60));
    assert_eq!(parse("99999999999999999999999")?, Duration::from_millis(u64::MAX));
    for (value, message) in [
        ("1.5", "invalid literal for int() with base 10: '1.5'"),
        ("", "invalid literal for int() with base 10: ''"),
        ("1__0", "invalid literal for int() with base 10: '1__0'"),
        ("it's", "invalid literal for int() with base 10: \"it's\""),
        ("a\tb\\", "invalid literal for int() with base 10: 'a\\tb\\\\'"),
        ("\u{1}", "invalid literal for int() with base 10: '\\x01'"),
    ] {
        assert_eq!(parse(value).map_err(|error| error.to_string()), Err(message.to_owned()));
    }
    Ok(())
}

fn settings(text: &str) -> Option<crate::Settings> {
    crate::Settings::from_json(text).ok()
}

fn initialize_golden(name: &str, options: ClaudeAgentOptions) -> Result<(), Error> {
    let options = ClaudeAgentOptions {
        argv_style: crate::ArgvStyle::TypeScript,
        ..options
    };
    let (request, _, _) = initialize_config(&options)?;
    let all: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/settings_option/goldens.json")).unwrap_or_default();
    let want = all["typescript"][name]["initialize"].clone();
    assert_eq!(request, want, "{name}");
    assert_eq!(request.to_string(), want.to_string(), "{name}");
    Ok(())
}

#[test]
fn the_initialize_request_carries_managed_settings_after_the_plugins() -> Result<(), Error> {
    let base = ClaudeAgentOptions {
        managed_settings_delivery: crate::ManagedSettingsDelivery::Initialize,
        ..Default::default()
    };
    initialize_golden(
        "managed_initialize",
        ClaudeAgentOptions {
            managed_settings: settings(r#"{"permissions":{"allow":["Bash"]},"n":1}"#),
            ..base.clone()
        },
    )?;
    initialize_golden(
        "managed_initialize_plugins",
        ClaudeAgentOptions {
            managed_settings: settings(r#"{"n":1}"#),
            plugin_delivery: crate::PluginDelivery::Initialize,
            plugins: vec![crate::LocalPlugin::new("/p")],
            ..base.clone()
        },
    )?;
    initialize_golden("managed_initialize_without_settings", base)?;
    initialize_golden(
        "managed_argv",
        ClaudeAgentOptions {
            managed_settings: settings(r#"{"n":1}"#),
            ..Default::default()
        },
    )?;
    Ok(())
}
