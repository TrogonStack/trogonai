use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

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
    commands: mpsc::Sender<Command>,
    messages: mpsc::Receiver<Message>,
    shutdown: oneshot::Sender<()>,
}

impl DriverFixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_handler(None)
    }

    fn with_handler(handler: Option<Arc<dyn ToolPermissionHandler>>) -> Result<Self, Box<dyn std::error::Error>> {
        let (written_tx, written) = mpsc::unbounded_channel();
        let (incoming_tx, incoming) = mpsc::unbounded_channel();
        let (commands, command_rx) = mpsc::channel(4);
        let (message_tx, messages) = mpsc::channel(4);
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
            shutdown: shutdown_rx,
            limits: Limits::default(),
            handler,
            message_decoding: crate::MessageDecoding::TypeScript,
            pending: HashMap::new(),
            callbacks: JoinSet::new(),
            callback_handles: HashMap::new(),
            callback_signals: HashMap::new(),
            hook_callbacks: HashMap::new(),
            sdk_mcp_servers: BTreeMap::new(),
            pending_mcp: None,
            initialize_request: crate::wire::runtime_payload::InitializeRequest::default()
                .into_payload(crate::ArgvStyle::TypeScript)
                .to_value()?,
            initialize_expectations: InitializeExpectations::default(),
            version_check: None,
            elicitation_handler: None,
            user_dialog_handler: None,
            dialog_kinds: HashSet::new(),
            session: None,
            mirror_flush: None,
            deferred_messages: VecDeque::new(),
            next_id: 0,
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
    assert!(outcome.is_err(), "a stale completion emitted a response to the CLI");
    assert!(elapsed >= Duration::from_millis(100));
    assert!(elapsed <= Duration::from_millis(101));
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
    let (replacement, _receiver) = mpsc::channel(1);
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
            deadline: Instant::now(),
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
        Duration::from_secs(1),
        wire::callback_error_response(&id, "fallback"),
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
    let command = command(&options, std::path::Path::new("claude"), &Inherited::new())?;
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
    let command = command(options, cli, inherited).ok()?;
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
