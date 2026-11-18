#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use tokio::sync::mpsc;

use super::*;
use crate::{
    ToolPermissionRequest, ToolSet, environment_plan::Inherited, process_transport::command,
    python_c_stack::PythonCStack,
};

/// A transport the test scripts: documents the driver writes arrive on `written`, and objects sent on
/// `incoming` are what the driver reads.
struct ScriptedTransport {
    written: mpsc::UnboundedSender<String>,
    incoming: mpsc::UnboundedReceiver<JsonObject>,
    ready: bool,
    /// How long each write takes before the document arrives, as a busy pipe or a remote transport would.
    write_delay: Option<Duration>,
    /// The first half of each document arrives at once and the rest after `write_delay`, as a pipe takes a frame
    /// larger than its free space.
    split: bool,
    /// Each kill, end of input and close, in order.
    steps: Option<mpsc::UnboundedSender<&'static str>>,
    /// Takes each document whole or not at all, as the SSE upload queue does.
    whole: bool,
}

impl ScriptedTransport {
    fn step(&self, step: &'static str) {
        if let Some(steps) = &self.steps {
            let _ = steps.send(step);
        }
    }
}

impl Transport for ScriptedTransport {
    async fn connect(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn write(&mut self, data: &str) -> Result<(), Error> {
        let mut rest = data;
        if self.split {
            let (head, tail) = data.split_at(data.len() / 2);
            self.written.send(head.to_owned()).map_err(|_| Error::DriverStopped)?;
            rest = tail;
        }
        if let Some(delay) = self.write_delay {
            sleep(delay).await;
        }
        self.written.send(rest.to_owned()).map_err(|_| Error::DriverStopped)
    }

    async fn read_message(&mut self) -> Result<Option<JsonObject>, Error> {
        Ok(self.incoming.recv().await)
    }

    async fn close(&mut self) -> Result<(), Error> {
        self.step("close");
        self.ready = false;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn writes_whole_documents(&self) -> bool {
        self.whole
    }

    async fn end_input(&mut self) -> Result<(), Error> {
        self.step("end_input");
        self.ready = false;
        Ok(())
    }

    async fn kill(&mut self) -> Result<(), Error> {
        self.step("kill");
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
                write_delay: None,
                split: false,
                steps: None,
                whole: false,
            },
            input_ended: false,
            commands: command_rx,
            messages: message_tx,
            overflow: VecDeque::new(),
            shutdown: shutdown_rx,
            shutdown_seen: false,
            close_deadline: None,
            frame_cut: false,
            abort: crate::CancellationToken::new(),
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
            startup_tap: None,
            verbatim_prompts: false,
            warn_unsupported_verbatim: false,
            elicitation_handler: None,
            user_dialog_handler: None,
            oauth_token_handler: None,
            host_auth_token_handler: None,
            host: crate::remote_control::HostState::default(),
            session: None,
            mirror_flush: None,
            deferred_messages: VecDeque::new(),
            held_observations: VecDeque::new(),
            initialize_hold: false,
            prompt_hold: PromptHold::default(),
            held_commands: VecDeque::new(),
            next_id: 0,
            wire_ids: None,
            state: State::Idle,
            run_state: RunState::default(),
            run_end: crate::run_end::RunEnd::new(crate::ArgvStyle::TypeScript, Some(Duration::from_secs(600))),
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
    fixture.driver.argv_style = crate::ArgvStyle::Python;
    let (response, receiver) = oneshot::channel();
    fixture
        .driver
        .command(Command::Prompt {
            input: Box::new(crate::Prompt::from("before handshake").into()),
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

/// A fixture whose close was already requested before the driver writes, with writes that take `delay`.
fn closed_before_a_write(delay: Option<Duration>) -> Result<DriverFixture, Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.transport.write_delay = delay;
    let (replacement, _receiver) = oneshot::channel();
    let shutdown = std::mem::replace(&mut fixture.shutdown, replacement);
    shutdown.send(()).map_err(|_| Error::DriverStopped)?;
    Ok(fixture)
}

fn prompt_command(text: &str) -> (Command, oneshot::Receiver<Result<(), Error>>) {
    let (response, receiver) = oneshot::channel();
    let command = Command::Prompt {
        input: Box::new(crate::Prompt::from(text).into()),
        response,
    };
    (command, receiver)
}

/// The close lands after the driver took the prompt, as when `WarmQuery.query` returns and the caller disconnects at
/// once. A random choice between the close and a transport that is ready drops the prompt half the time, so every
/// round must write it.
#[tokio::test(start_paused = true)]
async fn a_prompt_taken_before_the_close_is_written() -> Result<(), Box<dyn std::error::Error>> {
    for round in 0..64 {
        let mut fixture = closed_before_a_write(None)?;
        let (command, receiver) = prompt_command("taken before the close");
        fixture.driver.command(command).await?;
        assert!(matches!(receiver.await?, Ok(())), "round {round}");
        let written = fixture.harness.written.try_recv()?;
        assert!(written.contains("taken before the close"), "round {round}: {written}");
        // The close still ends the run before anything else is read.
        fixture.driver.event_loop().await?;
        assert!(fixture.driver.shutdown_seen, "round {round}");
        fixture.finish().await?;
    }
    Ok(())
}

/// A transport that cannot take the frame loses to the close at the TypeScript close grace, so a CLI that stopped
/// reading cannot hold the close.
#[tokio::test(start_paused = true)]
async fn a_write_that_has_to_wait_gives_way_to_the_close_at_the_grace() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = closed_before_a_write(Some(Duration::from_secs(3600)))?;
    let (command, _receiver) = prompt_command("never drained");
    let started = Instant::now();
    let outcome = fixture.driver.command(command).await;
    assert!(matches!(outcome, Err(Error::ShutdownRequested)), "{outcome:?}");
    assert_eq!(started.elapsed(), constants::TYPESCRIPT_CLOSE_GRACE);
    assert!(fixture.driver.shutdown_seen);
    assert!(fixture.driver.frame_cut);
    fixture.finish().await
}

/// A transport that takes a frame whole or not at all has nothing of the frame on the wire while it waits, so the
/// close wins at once, as a JavaScript close does over a write still awaiting its transport.
#[tokio::test(start_paused = true)]
async fn a_write_to_a_whole_document_transport_gives_way_to_the_close_at_once() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = closed_before_a_write(Some(Duration::from_secs(3600)))?;
    fixture.driver.transport.whole = true;
    let (command, _receiver) = prompt_command("never sent");
    let started = Instant::now();
    let outcome = fixture.driver.command(command).await;
    assert!(matches!(outcome, Err(Error::ShutdownRequested)), "{outcome:?}");
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert!(fixture.driver.shutdown_seen);
    assert!(!fixture.driver.frame_cut);
    assert!(fixture.driver.close_deadline.is_none());
    fixture.finish().await
}

/// A fixture whose close was already requested, with a pipe that takes half of each frame at once and the rest
/// after `delay`; the receiver yields the transport's kill, end of input and close in order.
fn split_pipe(
    delay: Duration,
) -> Result<(DriverFixture, mpsc::UnboundedReceiver<&'static str>), Box<dyn std::error::Error>> {
    let mut fixture = closed_before_a_write(Some(delay))?;
    fixture.driver.transport.split = true;
    let (steps, stepped) = mpsc::unbounded_channel();
    fixture.driver.transport.steps = Some(steps);
    Ok((fixture, stepped))
}

/// Everything the scripted pipe took so far, as the CLI would read it from stdin.
fn stdin_bytes(written: &mut mpsc::UnboundedReceiver<String>) -> String {
    let mut stdin = String::new();
    while let Ok(chunk) = written.try_recv() {
        stdin.push_str(&chunk);
    }
    stdin
}

/// The close lands after half of the frame is on stdin. `sdk.mjs` has handed the whole frame to the stdin stream by
/// then and Python's close waits for the write in progress, so the frame is finished before the close is honored
/// instead of leaving half a line.
#[tokio::test(start_paused = true)]
async fn a_frame_the_close_meets_halfway_is_written_whole() -> Result<(), Box<dyn std::error::Error>> {
    let (mut fixture, mut steps) = split_pipe(Duration::from_secs(1))?;
    let (command, receiver) = prompt_command("met halfway");
    let started = Instant::now();
    fixture.driver.command(command).await?;
    assert!(matches!(receiver.await?, Ok(())));
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert!(fixture.driver.shutdown_seen);
    assert!(!fixture.driver.frame_cut);
    let stdin = stdin_bytes(&mut fixture.harness.written);
    let frame: serde_json::Value = serde_json::from_str(stdin.strip_suffix('\n').ok_or("no newline")?)?;
    assert_eq!(frame["message"]["content"], "met halfway");
    // The close still ends the run before anything else is read.
    fixture.driver.event_loop().await?;
    assert!(steps.try_recv().is_err(), "the transport was stopped");
    fixture.finish().await
}

/// A queued prompt the CLI stops draining partway ends the flush at each SDK's close grace: TypeScript stops the
/// process 2 seconds after `close()`, Python gives up on the write lock after 5. The process is stopped before input
/// ends, so the CLI never reads the half line with an end of file behind it, and the prompt behind it is not written.
#[tokio::test(start_paused = true)]
async fn a_pipe_that_stops_draining_ends_the_queued_prompts_at_the_close_grace()
-> Result<(), Box<dyn std::error::Error>> {
    for (style, grace) in [
        (crate::ArgvStyle::TypeScript, constants::TYPESCRIPT_CLOSE_GRACE),
        (crate::ArgvStyle::Python, constants::PYTHON_CLOSE_WAIT),
    ] {
        let (mut fixture, mut steps) = split_pipe(Duration::from_secs(3600))?;
        fixture.driver.argv_style = style;
        let (stuck, stuck_written) = prompt_command("never drained");
        let (behind, _behind_written) = prompt_command("queued behind");
        for command in [stuck, behind] {
            fixture.commands.try_send(command).map_err(|_| Error::DriverStopped)?;
        }
        let (terminal, terminated) = oneshot::channel();
        let started = Instant::now();
        fixture.driver.run(Ok(()), terminal).await;
        assert_eq!(started.elapsed(), grace, "{style:?}");
        assert!(matches!(terminated.await?, Ok(())), "{style:?}");
        assert!(!matches!(stuck_written.await, Ok(Ok(()))), "{style:?}");
        let stdin = stdin_bytes(&mut fixture.harness.written);
        assert!(!stdin.is_empty() && !stdin.contains('\n'), "{style:?}: {stdin}");
        assert!(!stdin.contains("queued behind"), "{style:?}: {stdin}");
        assert_eq!(recorded_steps(&mut steps), ["kill", "end_input", "close"], "{style:?}");
    }
    Ok(())
}

/// The first queued prompt finishes exactly at each SDK's close grace, so the frame already under way still wins
/// over the deadline and arrives whole. The prompt behind it would start after the grace, when TypeScript has stopped
/// the process and Python has marked the transport not ready, so none of it reaches stdin and nothing is cut.
#[tokio::test(start_paused = true)]
async fn no_new_frame_starts_after_the_close_grace() -> Result<(), Box<dyn std::error::Error>> {
    for (style, grace) in [
        (crate::ArgvStyle::TypeScript, constants::TYPESCRIPT_CLOSE_GRACE),
        (crate::ArgvStyle::Python, constants::PYTHON_CLOSE_WAIT),
    ] {
        let (mut fixture, mut steps) = split_pipe(grace)?;
        fixture.driver.argv_style = style;
        let (on_time, on_time_written) = prompt_command("finished at the grace");
        let (late, late_written) = prompt_command("started after the grace");
        for command in [on_time, late] {
            fixture.commands.try_send(command).map_err(|_| Error::DriverStopped)?;
        }
        let (terminal, terminated) = oneshot::channel();
        let started = Instant::now();
        fixture.driver.run(Ok(()), terminal).await;
        assert_eq!(started.elapsed(), grace, "{style:?}");
        assert!(matches!(terminated.await?, Ok(())), "{style:?}");
        assert!(matches!(on_time_written.await, Ok(Ok(()))), "{style:?}");
        assert!(!matches!(late_written.await, Ok(Ok(()))), "{style:?}");
        let stdin = stdin_bytes(&mut fixture.harness.written);
        let frame: serde_json::Value = serde_json::from_str(stdin.strip_suffix('\n').ok_or("no newline")?)?;
        assert_eq!(frame["message"]["content"], "finished at the grace", "{style:?}");
        assert!(!stdin.contains("started after the grace"), "{style:?}: {stdin}");
        assert_eq!(recorded_steps(&mut steps), ["end_input", "close"], "{style:?}");
    }
    Ok(())
}

/// The steps the scripted transport recorded so far, in order.
fn recorded_steps(steps: &mut mpsc::UnboundedReceiver<&'static str>) -> Vec<&'static str> {
    let mut order = Vec::new();
    while let Ok(step) = steps.try_recv() {
        order.push(step);
    }
    order
}

/// The close lands while the event loop is writing a prompt the CLI stopped draining partway. The frame is waited for
/// only until each SDK's close grace, and the run still ends cleanly: TypeScript stops the process 2 seconds after
/// `close()`, Python gives up on the write lock after 5. The process is stopped before input ends, so the CLI never
/// reads the half line with an end of file behind it, and a prompt queued behind it is not written after the cut.
#[tokio::test(start_paused = true)]
async fn a_close_that_meets_a_stalled_write_ends_the_run_cleanly_at_the_close_grace()
-> Result<(), Box<dyn std::error::Error>> {
    for (style, grace) in [
        (crate::ArgvStyle::TypeScript, constants::TYPESCRIPT_CLOSE_GRACE),
        (crate::ArgvStyle::Python, constants::PYTHON_CLOSE_WAIT),
    ] {
        let mut fixture = DriverFixture::new()?;
        fixture.driver.argv_style = style;
        fixture.driver.transport.write_delay = Some(Duration::from_secs(3600));
        fixture.driver.transport.split = true;
        let (steps, mut stepped) = mpsc::unbounded_channel();
        fixture.driver.transport.steps = Some(steps);
        let (stuck, stuck_written) = prompt_command("never drained");
        let (behind, _behind_written) = prompt_command("queued behind");
        for command in [stuck, behind] {
            fixture.commands.try_send(command).map_err(|_| Error::DriverStopped)?;
        }
        let DriverFixture {
            driver,
            mut harness,
            commands: _commands,
            messages: _messages,
            shutdown,
        } = fixture;
        let (terminal, terminated) = oneshot::channel();
        let started = Instant::now();
        // The close is requested once the first half of the frame is on stdin, so the write is already under way.
        let close = async {
            let head = harness.written.recv().await;
            let sent = shutdown.send(());
            (head, sent)
        };
        let ((head, sent), ()) = tokio::join!(close, driver.run(Ok(()), terminal));
        assert!(sent.is_ok(), "{style:?}: the run ended before the close");
        assert_eq!(started.elapsed(), grace, "{style:?}");
        assert!(matches!(terminated.await?, Ok(())), "{style:?}");
        assert!(!matches!(stuck_written.await, Ok(Ok(()))), "{style:?}");
        let mut stdin = head.ok_or("nothing reached stdin")?;
        stdin.push_str(&stdin_bytes(&mut harness.written));
        assert!(!stdin.is_empty(), "{style:?}");
        assert!(!stdin.contains('\n'), "{style:?}: {stdin}");
        assert!(!stdin.contains("queued behind"), "{style:?}: {stdin}");
        assert_eq!(
            recorded_steps(&mut stepped),
            ["kill", "end_input", "close"],
            "{style:?}"
        );
    }
    Ok(())
}

/// A write that finishes inside the opt-in write timeout is not cut short: the frame arrives whole and the prompt is
/// taken.
#[tokio::test(start_paused = true)]
async fn a_write_inside_the_write_timeout_delivers_the_whole_frame() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.transport.write_delay = Some(Duration::from_secs(1));
    fixture.driver.transport.split = true;
    fixture.driver.limits.write_timeout = Some(crate::Timeout::new(Duration::from_secs(5)));
    let (command, receiver) = prompt_command("in time");
    let started = Instant::now();
    fixture.driver.command(command).await?;
    assert!(matches!(receiver.await?, Ok(())));
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert!(!fixture.driver.frame_cut);
    let stdin = stdin_bytes(&mut fixture.harness.written);
    let frame: serde_json::Value = serde_json::from_str(stdin.strip_suffix('\n').ok_or("no newline")?)?;
    assert_eq!(frame["message"]["content"], "in time");
    fixture.finish().await
}

/// A pipe that stops draining partway through a frame fails the run with `WriteTimeout` once the opt-in write timeout
/// passes. Half a line is on stdin, so the process is stopped before input ends and the CLI never reads it as a
/// finished frame.
#[tokio::test(start_paused = true)]
async fn a_write_past_the_write_timeout_fails_the_run_and_stops_the_process_before_input_ends()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let limit = Duration::from_secs(5);
    fixture.driver.transport.write_delay = Some(Duration::from_secs(3600));
    fixture.driver.transport.split = true;
    let (steps, mut stepped) = mpsc::unbounded_channel();
    fixture.driver.transport.steps = Some(steps);
    fixture.driver.limits.write_timeout = Some(crate::Timeout::new(limit));
    let (stuck, stuck_written) = prompt_command("never drained");
    fixture.commands.try_send(stuck).map_err(|_| Error::DriverStopped)?;
    let (terminal, terminated) = oneshot::channel();
    let started = Instant::now();
    fixture.driver.run(Ok(()), terminal).await;
    assert_eq!(started.elapsed(), limit);
    let outcome = terminated.await?;
    assert!(matches!(outcome, Err(Error::WriteTimeout)), "{outcome:?}");
    assert!(!matches!(stuck_written.await, Ok(Ok(()))));
    let stdin = stdin_bytes(&mut fixture.harness.written);
    assert!(!stdin.is_empty() && !stdin.contains('\n'), "{stdin}");
    assert_eq!(recorded_steps(&mut stepped), ["kill", "end_input", "close"]);
    Ok(())
}

/// The close reached the driver ahead of commands queued before it: the prompts go to stdin in order and everything
/// else is handed back unapplied.
#[tokio::test(start_paused = true)]
async fn prompts_queued_behind_the_close_are_written_and_the_rest_handed_back() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = closed_before_a_write(None)?;
    fixture.driver.shutdown_seen = true;
    let (first, first_written) = prompt_command("queued first");
    let (initialize, _initialized) = oneshot::channel();
    let (second, second_written) = prompt_command("queued second");
    for command in [
        first,
        Command::Initialize {
            first: false,
            response: initialize,
        },
        second,
    ] {
        fixture.commands.try_send(command).map_err(|_| Error::DriverStopped)?;
    }
    let unapplied = fixture.driver.write_queued_prompts().await;
    assert!(matches!(
        unapplied.as_slice(),
        [Command::Initialize { first: false, .. }]
    ));
    assert!(matches!(first_written.await?, Ok(())));
    assert!(matches!(second_written.await?, Ok(())));
    for expected in ["queued first", "queued second"] {
        let written = fixture.harness.written.try_recv()?;
        assert!(written.contains(expected), "{written}");
    }
    fixture.finish().await
}

/// A queued prompt the transport cannot take stops the flush, so the prompts behind it stay unwritten.
#[tokio::test(start_paused = true)]
async fn a_queued_prompt_that_cannot_be_written_stops_the_flush() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = closed_before_a_write(Some(Duration::from_secs(3600)))?;
    fixture.driver.shutdown_seen = true;
    fixture.driver.abort.cancel();
    let (stuck, _stuck_written) = prompt_command("stuck");
    let (behind, _behind_written) = prompt_command("behind");
    for command in [stuck, behind] {
        fixture.commands.try_send(command).map_err(|_| Error::DriverStopped)?;
    }
    let started = Instant::now();
    let unapplied = fixture.driver.write_queued_prompts().await;
    assert!(unapplied.is_empty());
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert!(matches!(
        fixture.driver.commands.try_recv(),
        Some(Command::Prompt { .. })
    ));
    fixture.finish().await
}

#[tokio::test(start_paused = true)]
async fn shutdown_queue_flushes_prompts_in_order_without_applying_other_commands()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let (control, mut controlled) = oneshot::channel();
    let (first, first_written) = oneshot::channel();
    let (finish, mut input_finished) = oneshot::channel();
    let (second, second_written) = oneshot::channel();
    for command in [
        Command::Control {
            operation: ControlOperation::Interrupt,
            request: serde_json::json!({"subtype": "interrupt"}),
            response: control,
        },
        Command::Prompt {
            input: Box::new(crate::Prompt::from("first queued prompt").into()),
            response: first,
        },
        Command::FinishInput {
            completion: Completion::Run,
            written: None,
            response: finish,
        },
        Command::Prompt {
            input: Box::new(crate::Prompt::from("second queued prompt").into()),
            response: second,
        },
    ] {
        assert!(fixture.commands.try_send(command).is_ok());
    }
    fixture.shutdown.send(()).map_err(|_| Error::DriverStopped)?;
    let (terminal, terminated) = oneshot::channel();
    fixture.driver.run(Ok(()), terminal).await;

    assert!(matches!(terminated.await?, Ok(())));
    assert!(matches!(first_written.await?, Ok(())));
    assert!(matches!(second_written.await?, Ok(())));
    for expected in ["first queued prompt", "second queued prompt"] {
        let frame: serde_json::Value = serde_json::from_str(&fixture.harness.written.try_recv()?)?;
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "user",
                "message": {"role": "user", "content": expected},
                "parent_tool_use_id": null,
            })
        );
    }
    assert!(fixture.harness.written.recv().await.is_none());
    assert!(matches!(
        controlled.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(matches!(
        input_finished.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn shutdown_queue_stops_at_first_write_failure_without_draining_later_commands()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    fixture.driver.shutdown_seen = true;
    fixture.harness.written.close();
    let (control, mut controlled) = oneshot::channel();
    let (first, mut first_written) = oneshot::channel();
    let (second, mut second_written) = oneshot::channel();
    let (finish, mut input_finished) = oneshot::channel();
    for command in [
        Command::Control {
            operation: ControlOperation::Interrupt,
            request: serde_json::json!({"subtype": "interrupt"}),
            response: control,
        },
        Command::Prompt {
            input: Box::new(crate::Prompt::from("failed queued prompt").into()),
            response: first,
        },
        Command::Prompt {
            input: Box::new(crate::Prompt::from("later queued prompt").into()),
            response: second,
        },
        Command::FinishInput {
            completion: Completion::Run,
            written: None,
            response: finish,
        },
    ] {
        assert!(fixture.commands.try_send(command).is_ok());
    }

    let unapplied = fixture.driver.write_queued_prompts().await;

    assert!(matches!(
        unapplied.as_slice(),
        [Command::Control {
            operation: ControlOperation::Interrupt,
            ..
        }]
    ));
    assert!(matches!(
        controlled.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        first_written.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(matches!(
        second_written.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        input_finished.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        fixture.driver.commands.try_recv(),
        Some(Command::Prompt { input, .. }) if input.to_wire_json()?["message"]["content"] == "later queued prompt"
    ));
    assert!(matches!(
        fixture.driver.commands.try_recv(),
        Some(Command::FinishInput { .. })
    ));
    assert!(fixture.driver.commands.try_recv().is_none());
    assert_eq!(fixture.driver.next_id, 0);
    assert!(fixture.driver.run_state.input_done.is_none());
    drop(unapplied);
    assert!(matches!(
        controlled.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
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
    fixture.driver.spawn_callback::<_, serde_json::Value>(
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

/// Both upstream readers wait for the transcript flush before they handle a result, so the run ends, and stdin
/// closes, only after it; a frame after the result waits behind it.
#[tokio::test]
async fn a_result_reaches_the_run_end_machine_only_after_its_flush() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = DriverFixture::new()?;
    let decode = |value: serde_json::Value| {
        let raw = value.as_object().cloned().unwrap_or_default();
        crate::message_decode::typed(&raw, crate::MessageDecoding::TypeScript)
    };
    let result = decode(serde_json::json!({
        "type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "duration_api_ms": 1,
        "num_turns": 1, "session_id": "s", "result": "done",
    }));
    let later = decode(serde_json::json!({
        "type": "system", "subtype": "session_state_changed", "state": "idle", "session_id": "s",
    }));
    fixture.driver.run_end.prompt_written();
    fixture.driver.run_end.finish(false, true, None);
    fixture.driver.held_observations.push_back((result, true));
    fixture.driver.held_observations.push_back((later, false));
    assert!(!fixture.driver.run_end.close_due());
    fixture.driver.release_observations();
    assert!(fixture.driver.held_observations.is_empty());
    assert!(fixture.driver.run_end.close_due());
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
    let command = command(
        &options,
        std::path::Path::new("claude"),
        &Inherited::new(),
        None,
        PythonCStack::CLIENT_CONNECT,
    )?;
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
    let command = command(options, cli, inherited, None, PythonCStack::CLIENT_CONNECT).ok()?;
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
    let id = fixture.driver.next_request_id()?;
    assert!(id.as_str().starts_with("req_0_"));
    assert_eq!(fixture.driver.next_id, 0);
    let id = fixture.driver.next_request_id()?;
    assert!(id.as_str().starts_with("req_1_"));
    assert_eq!(fixture.driver.next_id, 1);
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

/// What the TypeScript SDK shows for a request that waited behind an initialize that never answered: the
/// timer's own message, and nothing held was ever written.
#[tokio::test(start_paused = true)]
async fn an_unanswered_initialize_expires_after_the_typescript_timer_and_rejects_what_was_held()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(TYPESCRIPT_INITIALIZE_TIMEOUT, Duration::from_secs(660));
    for (blocking_hooks, expected) in [
        (
            true,
            "Claude Code did not answer initialize, so a hook set to onFailure: \"block\" could not be confirmed and the prompt was not sent. Check the connection and start a new session.",
        ),
        (
            false,
            "Claude Code did not answer initialize, so the managed settings could not be confirmed and the prompt was not sent. Start a new session.",
        ),
    ] {
        let mut fixture = DriverFixture::new()?;
        fixture.driver.initialize_hold = true;
        fixture.driver.initialize_expectations.blocking_hooks = blocking_hooks;
        fixture.driver.initialize_expectations.managed_settings = !blocking_hooks;
        fixture.driver.initialize_timeout = Some(TYPESCRIPT_INITIALIZE_TIMEOUT);
        let (initialize, initialized) = oneshot::channel();
        let (prompt, prompted) = oneshot::channel();
        let (control, controlled) = oneshot::channel();
        let started = Instant::now();
        fixture.driver.pending.insert(
            ControlRequestId::from("initialize"),
            PendingControl {
                response: ControlResponder::Initialize {
                    first: true,
                    response: initialize,
                },
                deadline: fixture.driver.control_deadline(ControlOperation::Initialize),
            },
        );
        fixture.driver.held_commands.push_back(Command::Prompt {
            input: Box::new(crate::Prompt::from("hi").into()),
            response: prompt,
        });
        fixture.driver.held_commands.push_back(Command::Control {
            operation: ControlOperation::Interrupt,
            request: serde_json::json!({"subtype": "interrupt"}),
            response: control,
        });
        let outcome = fixture.driver.event_loop().await;
        assert_eq!(started.elapsed(), TYPESCRIPT_INITIALIZE_TIMEOUT);
        assert_eq!(outcome.err().map(|error| error.to_string()).as_deref(), Some(expected));
        assert_eq!(
            initialized.await?.err().map(|error| error.to_string()).as_deref(),
            Some(expected)
        );
        // The prompt is dropped unsent, and the held control fails with the same message.
        assert!(matches!(prompted.await?, Ok(())));
        let controlled = controlled.await?;
        assert!(
            matches!(controlled, Err(Error::InitializationFailed(_))),
            "{controlled:?}"
        );
        assert_eq!(
            controlled.err().map(|error| error.to_string()).as_deref(),
            Some(expected)
        );
        assert!(fixture.driver.held_commands.is_empty());
        fixture.finish().await?;
    }
    Ok(())
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
            seen.push(message.subtype.clone());
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
        pending::<Result<Option<serde_json::Value>, ProtocolError>>(),
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
    assert_eq!(python_initialize_timeout(None)?, Duration::from_secs(60));
    assert_eq!(
        python_initialize_timeout(Some(" +120_500\n".into()))?,
        Duration::from_millis(120_500)
    );
    assert_eq!(python_initialize_timeout(Some("1".repeat(308).into()))?, Duration::MAX);
    assert!(matches!(
        python_initialize_timeout(Some("1.5".into())),
        Err(Error::InvalidStreamCloseTimeout { .. })
    ));
    Ok(())
}

/// Replays what the Python SDK's client computed for `initialize_timeout` from each captured variable value.
#[cfg(unix)]
#[test]
fn python_initialize_seconds_match_the_captured_goldens() -> Result<(), Box<dyn std::error::Error>> {
    let goldens: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/python_text/goldens.json"))?;
    let cases = goldens["stream_close_timeout"]
        .as_array()
        .ok_or("no stream_close_timeout goldens")?;
    assert_eq!(cases.len(), 61);
    for case in cases {
        let bytes = case["value"].as_str().map(hex_bytes).transpose()?;
        let result = python_initialize_seconds(bytes.as_deref().map(OsStr::from_bytes));
        if let Some(seconds) = case["seconds"].as_str() {
            assert_eq!(
                result.ok().map(f64::to_bits),
                Some(seconds.parse::<f64>()?.to_bits()),
                "{case}"
            );
            continue;
        }
        let kind = result.as_ref().err().map(|error| {
            if matches!(error, Error::StreamCloseTimeoutOverflow) {
                "OverflowError"
            } else {
                "ValueError"
            }
        });
        assert_eq!(kind, case["error_type"].as_str(), "{case}");
        assert_eq!(
            result.err().map(|error| error.to_string()).as_deref(),
            case["error"].as_str(),
            "{case}"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn hex_bytes(hex: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(hex.get(index..index + 2).unwrap_or_default(), 16))
        .collect()
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

/// Deeper than the native stack of a test thread holds a recursive walk over.
const DEEP: usize = 200_000;

/// `levels` arrays around `null`, built without recursion.
fn deep_value(levels: usize) -> serde_json::Value {
    (0..levels).fold(serde_json::Value::Null, |inner, _| {
        serde_json::Value::Array(vec![inner])
    })
}

/// How many arrays `value` opens before its first item that is not one.
fn array_depth(mut value: &serde_json::Value) -> usize {
    let mut levels = 0;
    while let Some(inner) = value.as_array().and_then(|items| items.first()) {
        levels += 1;
        value = inner;
    }
    levels
}

#[tokio::test(start_paused = true)]
async fn a_message_nested_past_the_native_stack_is_decoded_routed_and_dropped() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = DriverFixture::new()?;
    let field = |key: &str, value: serde_json::Value| (key.to_owned(), value);
    let result = JsonObject::from_iter([
        field("type", "result".into()),
        field("subtype", "error_during_execution".into()),
        field("duration_ms", 1.into()),
        field("duration_api_ms", 1.into()),
        field("is_error", true.into()),
        field("num_turns", 1.into()),
        field("session_id", "s".into()),
        field("extra", deep_value(DEEP)),
    ]);
    let system = JsonObject::from_iter([
        field("type", "system".into()),
        field("subtype", "deep".into()),
        field("payload", deep_value(DEEP)),
    ]);
    for object in [result, system] {
        let frame = fixture.driver.decode_message(object)?;
        fixture.driver.frame(frame).await?;
    }
    assert!(fixture.driver.last_error_result.is_none());
    let Some(Message::Result(result)) = fixture.messages.recv().await else {
        return Err("expected the result".into());
    };
    assert_eq!(result.additional_fields.get("extra").map(array_depth), Some(DEEP));
    let Some(Message::System(system)) = fixture.messages.recv().await else {
        return Err("expected the system message".into());
    };
    assert_eq!(system.data.get("payload").map(array_depth), Some(DEEP));
    drop((result, system));
    fixture.finish().await
}

/// Python `close()` leaves each unanswered request to run out its own timeout, whatever kind of request it is, and
/// one sent without a timeout waits until the caller stops waiting, never answering on its own.
#[tokio::test(start_paused = true)]
async fn requests_left_at_python_close_run_out_their_own_timeouts() -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let (response, initialize) = oneshot::channel();
    ControlResponder::Initialize { first: true, response }.time_out_at(Some(deadline));
    let (response, control) = oneshot::channel();
    ControlResponder::Control {
        operation: ControlOperation::SetModel,
        response,
    }
    .time_out_at(Some(deadline));
    assert!(matches!(
        initialize.await?,
        Err(Error::ControlTimeout {
            operation: ControlOperation::Initialize
        })
    ));
    assert!(matches!(
        control.await?,
        Err(Error::ControlTimeout {
            operation: ControlOperation::SetModel
        })
    ));
    assert!(Instant::now() >= deadline);

    let (response, mut raw) = oneshot::channel();
    ControlResponder::Raw {
        operation: ControlOperation::Interrupt,
        response,
    }
    .time_out_at(None);
    tokio::time::advance(Duration::from_secs(86_400)).await;
    assert!(matches!(raw.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    drop(raw);
    let metrics = tokio::runtime::Handle::current().metrics();
    for _ in 0..8 {
        if metrics.num_alive_tasks() == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(metrics.num_alive_tasks(), 0);
    Ok(())
}
