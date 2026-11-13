use std::{
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::io::DuplexStream;

use super::*;
use crate::framing::drain_stderr;
use crate::{
    ToolPermissionRequest, ToolSet,
    adapters::{SpawnProcess, TokioProcessSpawner},
};

/// The far ends of the pipes a driver under test owns.
struct ProcessHarness {
    stdin: DuplexStream,
    _stdout: DuplexStream,
    _stderr: DuplexStream,
}

struct ProcessPipes {
    child: tokio::process::Child,
    stdin: DuplexStream,
    stdout: DuplexStream,
    stderr: DuplexStream,
}

/// Spawns a real short-lived child for the driver to own and pairs it with
/// in-memory pipes the test controls.
fn process() -> Result<(ProcessPipes, ProcessHarness), Box<dyn std::error::Error>> {
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command
        .arg("--list")
        // The driver kills this child, possibly while it writes its coverage profile at exit, which
        // leaves a corrupt profile that fails the merge. The profile is discarded.
        .env("LLVM_PROFILE_FILE", "/dev/null")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = TokioProcessSpawner.spawn(&mut command)?;
    let (stdin, input) = tokio::io::duplex(65536);
    let (output, stdout) = tokio::io::duplex(65536);
    let (error, stderr) = tokio::io::duplex(65536);
    Ok((
        ProcessPipes {
            child,
            stdin,
            stdout,
            stderr,
        },
        ProcessHarness {
            stdin: input,
            _stdout: output,
            _stderr: error,
        },
    ))
}

#[tokio::test(start_paused = true)]
async fn stale_permission_completion_cannot_remove_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let (
        ProcessPipes {
            child,
            stdin,
            stdout,
            stderr,
        },
        mut harness,
    ) = process()?;
    let (_command_tx, command_rx) = mpsc::channel(4);
    let (message_tx, _message_rx) = mpsc::channel(4);
    let (_shutdown_tx, shutdown_rx) = oneshot::channel();
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
    let mut driver = Driver::<_, _, crate::adapters::NativeFileSystem> {
        child,
        stdin: Some(stdin),
        frames: FrameReader::new(stdout, 1024),
        stderr: tokio::spawn(drain_stderr(stderr, 1024)),
        commands: command_rx,
        messages: message_tx,
        shutdown: shutdown_rx,
        limits: Limits::default(),
        handler: Some(Arc::new(handler)),
        pending: HashMap::new(),
        callbacks: JoinSet::new(),
        callback_handles: HashMap::new(),
        callback_signals: HashMap::new(),
        hook_callbacks: HashMap::new(),
        sdk_mcp_servers: BTreeMap::new(),
        pending_mcp: None,
        initialize_request: RequestPayload::Initialize {
            hooks: None,
            agents: None,
            supported_dialog_kinds: None,
            system_prompt: None,
            append_system_prompt: None,
        }
        .to_value()?,
        blocking_hooks: false,
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
        closed: Default::default(),
    };
    let id = ControlRequestId::from("reused-id");
    let permission = || -> Result<IncomingControl, ProtocolError> {
        match wire::parse_frame(br#"{"type":"control_request","request_id":"reused-id","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{}}}"#)? {
            Frame::ControlRequest { request, .. } => Ok(request),
            _ => Err(ProtocolError::DuplicateControlRequest),
        }
    };
    driver
        .frame(Frame::ControlRequest {
            id: id.clone(),
            request: permission()?,
        })
        .await?;
    timeout(Duration::from_secs(1), async {
        while !driver.callback_handles.get(&id).is_some_and(AbortHandle::is_finished) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    driver.frame(Frame::Cancel { id: id.clone() }).await?;
    driver
        .frame(Frame::ControlRequest {
            id: id.clone(),
            request: permission()?,
        })
        .await?;
    let started = Instant::now();
    let outcome = timeout(Duration::from_millis(100), driver.event_loop()).await;
    let elapsed = started.elapsed();
    let replacement_retained = driver.callback_handles.contains_key(&id);
    driver.callbacks.abort_all();
    driver.stdin.take();
    driver.child.start_kill()?;
    driver.child.wait().await?;
    driver.stderr.abort();
    let mut written = Vec::new();
    timeout(
        Duration::from_secs(1),
        tokio::io::AsyncReadExt::read_to_end(&mut harness.stdin, &mut written),
    )
    .await??;
    assert!(
        replacement_retained,
        "a stale allow completion removed the replacement callback"
    );
    assert!(outcome.is_err(), "a stale completion emitted a response to the child");
    assert!(written.is_empty(), "a stale completion wrote a response to the child");
    assert!(elapsed >= Duration::from_millis(100));
    assert!(elapsed <= Duration::from_millis(101));
    Ok(())
}

struct DriverFixture {
    driver: Driver<DuplexStream, DuplexStream, crate::adapters::NativeFileSystem>,
    harness: ProcessHarness,
    commands: mpsc::Sender<Command>,
    messages: mpsc::Receiver<Message>,
    shutdown: oneshot::Sender<()>,
}

impl DriverFixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (
            ProcessPipes {
                child,
                stdin,
                stdout,
                stderr,
            },
            harness,
        ) = process()?;
        let (commands, command_rx) = mpsc::channel(4);
        let (message_tx, messages) = mpsc::channel(4);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let driver = Driver {
            child,
            stdin: Some(stdin),
            frames: FrameReader::new(stdout, 1024),
            stderr: tokio::spawn(drain_stderr(stderr, 1024)),
            commands: command_rx,
            messages: message_tx,
            shutdown: shutdown_rx,
            limits: Limits::default(),
            handler: None,
            pending: HashMap::new(),
            callbacks: JoinSet::new(),
            callback_handles: HashMap::new(),
            callback_signals: HashMap::new(),
            hook_callbacks: HashMap::new(),
            sdk_mcp_servers: BTreeMap::new(),
            pending_mcp: None,
            initialize_request: RequestPayload::Initialize {
                hooks: None,
                agents: None,
                supported_dialog_kinds: None,
                system_prompt: None,
                append_system_prompt: None,
            }
            .to_value()?,
            blocking_hooks: false,
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
            closed: Default::default(),
        };
        Ok(Self {
            driver,
            harness,
            commands,
            messages,
            shutdown,
        })
    }

    async fn finish(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.driver.callbacks.abort_all();
        self.driver.stdin.take();
        self.driver.child.start_kill()?;
        self.driver.child.wait().await?;
        self.driver.stderr.abort();
        let mut bytes = Vec::new();
        timeout(
            Duration::from_secs(1),
            tokio::io::AsyncReadExt::read_to_end(&mut self.harness.stdin, &mut bytes),
        )
        .await??;
        assert!(bytes.is_empty(), "a defensive operation wrote to the process");
        Ok(())
    }
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
