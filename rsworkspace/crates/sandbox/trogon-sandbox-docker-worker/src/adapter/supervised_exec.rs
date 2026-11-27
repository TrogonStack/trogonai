use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures::StreamExt;
use sandbox_command_supervisor::protocol::{self, Certainty, CommandWire, Delivery, EventWire, ExecutionId, Identity};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    CommandDeadline, CommandEnv, CommandExecution, CommandOutput, CommandTimeoutStatus, OutputRetention,
    OutputTruncation, RunCommandRequest,
};
use trogon_sandbox_protocol::CappedOutput;
use trogon_sandbox_protocol::RedactionSet;

use super::{
    ChunkSequencer, CommandTask, DockerWorkerProvider, ExecCreatedCallback, OutputSink, OutputTap,
    STREAM_ONLY_STDERR_TAIL, StreamingContext,
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct Ready {
    pub helper: Identity,
    pub root: Identity,
    pub process_group: i32,
    pub session: i32,
}

#[derive(Clone)]
pub(super) struct Completion {
    pub released: bool,
    pub root_exit_code: Option<i32>,
    pub root_signal: Option<i32>,
    pub timed_out: bool,
    pub canceled: bool,
    pub reaped_count: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Clone, Default)]
struct State {
    hello: bool,
    ready: Option<Ready>,
    released: bool,
    completion: Option<Completion>,
    pending_signal: Option<(u64, i32)>,
    signal_result: Option<(u64, i32, bool)>,
    failed: bool,
}

struct Outgoing {
    command: CommandWire,
    acknowledged: oneshot::Sender<()>,
}

#[derive(Clone)]
pub(super) struct Controller {
    id: ExecutionId,
    commands: mpsc::Sender<Outgoing>,
    state: watch::Sender<State>,
    closed: CancellationToken,
    sequence: Arc<AtomicU64>,
    signal_gate: Arc<tokio::sync::Mutex<()>>,
}

pub(super) struct Launch {
    pub controller: Controller,
    receiver: mpsc::Receiver<Outgoing>,
    request: RunCommandRequest,
}

pub(super) fn prepare(
    id: &trogon_sandbox_contract::ExecutionId,
    request: RunCommandRequest,
) -> anyhow::Result<(Controller, Launch)> {
    let id = ExecutionId::new(id.as_str())?;
    let (commands, receiver) = mpsc::channel(16);
    let (state, _) = watch::channel(State::default());
    let controller = Controller {
        id,
        commands,
        state,
        closed: CancellationToken::new(),
        sequence: Arc::new(AtomicU64::new(1)),
        signal_gate: Arc::new(tokio::sync::Mutex::new(())),
    };
    Ok((
        controller.clone(),
        Launch {
            controller,
            receiver,
            request,
        },
    ))
}

impl Controller {
    pub fn close(&self) {
        self.closed.cancel();
    }

    async fn send(&self, command: CommandWire) -> anyhow::Result<()> {
        let checked = protocol::Command::try_from(command.clone())?;
        ensure!(
            checked.execution_id() == &self.id,
            "supervisor command identity changed"
        );
        let (acknowledged, written) = oneshot::channel();
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                biased;
                _ = self.closed.cancelled() => anyhow::bail!("supervisor control channel is closed"),
                result = self.commands.send(Outgoing { command, acknowledged }) => {
                    result.map_err(|_| anyhow::anyhow!("supervisor control channel is closed"))?;
                }
            }
            tokio::select! {
                biased;
                _ = self.closed.cancelled() => anyhow::bail!("supervisor control channel is closed"),
                result = written => result.context("supervisor command write is unconfirmed"),
            }
        })
        .await
        .context("supervisor control write deadline exceeded")
        .and_then(|result| result);
        if result.is_err() {
            self.close();
        }
        result
    }

    async fn wait<T>(&self, budget: Duration, result: impl Fn(&State) -> Option<T>) -> anyhow::Result<T> {
        let mut receiver = self.state.subscribe();
        tokio::time::timeout(budget, async {
            loop {
                let state = receiver.borrow_and_update().clone();
                if let Some(result) = result(&state) {
                    return Ok(result);
                }
                ensure!(
                    !state.failed && !self.closed.is_cancelled(),
                    "native supervisor custody remains unconfirmed"
                );
                receiver
                    .changed()
                    .await
                    .context("native supervisor state is unavailable")?;
            }
        })
        .await
        .context("native supervisor acknowledgement deadline exceeded")?
    }

    pub async fn ready(&self) -> anyhow::Result<Ready> {
        self.wait(Duration::from_secs(10), |state| state.ready.clone()).await
    }

    pub async fn release(&self) -> anyhow::Result<()> {
        self.send(CommandWire::Release {
            execution_id: self.id.clone(),
        })
        .await?;
        self.wait(Duration::from_secs(5), |state| state.released.then_some(()))
            .await
    }

    pub async fn signal(&self, signal: i32) -> anyhow::Result<bool> {
        let _gate = self.signal_gate.lock().await;
        if self.state.borrow().completion.is_some() {
            return Ok(false);
        }
        let mut sequence = self.sequence.load(Ordering::Acquire);
        loop {
            let next = sequence
                .checked_add(1)
                .context("supervisor signal sequence exhausted")?;
            match self
                .sequence
                .compare_exchange_weak(sequence, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(observed) => sequence = observed,
            }
        }
        self.state.send_modify(|state| {
            state.pending_signal = Some((sequence, signal));
            state.signal_result = None;
        });
        let sent = self
            .send(CommandWire::Signal {
                execution_id: self.id.clone(),
                request_id: sequence,
                signal,
            })
            .await;
        if let Err(error) = sent {
            return if self.completion().is_some() {
                Ok(false)
            } else {
                Err(error)
            };
        }
        self.wait(Duration::from_secs(10), |state| {
            state
                .signal_result
                .filter(|(id, observed, _)| *id == sequence && *observed == signal)
                .map(|(_, _, delivered)| delivered)
                .or_else(|| state.completion.as_ref().map(|_| false))
        })
        .await
    }

    pub async fn absent(&self) -> anyhow::Result<Completion> {
        self.wait(Duration::from_secs(10), |state| state.completion.clone())
            .await
    }

    pub fn released(&self) -> bool {
        self.state.borrow().released
    }

    pub fn completion(&self) -> Option<Completion> {
        self.state.borrow().completion.clone()
    }

    fn fail(&self) {
        self.state.send_modify(|state| state.failed = true);
        self.close();
    }

    fn same_id(&self, id: &ExecutionId) -> anyhow::Result<()> {
        ensure!(id == &self.id, "native supervisor response identity changed");
        Ok(())
    }
}

async fn writer(
    mut input: std::pin::Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
    mut receiver: mpsc::Receiver<Outgoing>,
    controller: Controller,
) {
    loop {
        let next = tokio::select! {
            biased;
            _ = controller.closed.cancelled() => break,
            next = receiver.recv() => next,
        };
        let Some(outgoing) = next else {
            break;
        };
        let result = async {
            let mut bytes = Vec::new();
            protocol::write_command(&mut bytes, &outgoing.command).context("encode private supervisor control")?;
            tokio::select! {
                biased;
                _ = controller.closed.cancelled() => anyhow::bail!("supervisor control channel is closed"),
                result = tokio::time::timeout(Duration::from_secs(5), async {
                    input.write_all(&bytes).await?;
                    input.flush().await
                }) => result.context("supervisor input write deadline exceeded")??,
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            controller.fail();
            break;
        }
        let _ = outgoing.acknowledged.send(());
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), input.shutdown()).await;
}

fn event_id(event: &EventWire) -> Option<&ExecutionId> {
    match event {
        EventWire::Hello { execution_id, .. }
        | EventWire::Ready { execution_id, .. }
        | EventWire::Released { execution_id }
        | EventWire::Output { execution_id, .. }
        | EventWire::RootExited { execution_id, .. }
        | EventWire::FamilyAbsent { execution_id, .. }
        | EventWire::Error { execution_id, .. }
        | EventWire::Status { execution_id, .. }
        | EventWire::SignalResult { execution_id, .. } => Some(execution_id),
        EventWire::Probe { .. } => None,
    }
}

fn request_deadline(request: &RunCommandRequest) -> CommandDeadline {
    match &request.execution {
        CommandExecution::Detached { deadline, .. } => *deadline,
        CommandExecution::Attached => CommandDeadline::default(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn collect(
    docker: &Docker,
    container: &str,
    command: Vec<String>,
    cwd: Option<String>,
    environment: &BTreeMap<String, String>,
    user: Option<&str>,
    callback: Option<ExecCreatedCallback>,
    streaming: Option<StreamingContext>,
    retention: OutputRetention,
    cap: Option<usize>,
    tap: Option<OutputTap>,
    redaction: &RedactionSet,
    launch: Launch,
) -> anyhow::Result<CommandOutput> {
    let controller = launch.controller.clone();
    let result = collect_owned(
        docker,
        container,
        command,
        cwd,
        environment,
        user,
        callback,
        streaming,
        retention,
        cap,
        tap,
        redaction,
        launch,
    )
    .await;
    if result.is_err() {
        controller.fail();
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn collect_owned(
    docker: &Docker,
    container: &str,
    command: Vec<String>,
    cwd: Option<String>,
    environment: &BTreeMap<String, String>,
    user: Option<&str>,
    callback: Option<ExecCreatedCallback>,
    streaming: Option<StreamingContext>,
    retention: OutputRetention,
    cap: Option<usize>,
    tap: Option<OutputTap>,
    redaction: &RedactionSet,
    launch: Launch,
) -> anyhow::Result<CommandOutput> {
    let Launch {
        controller,
        receiver,
        request,
    } = launch;
    let exec = docker
        .create_exec(
            container,
            CreateExecOptions {
                cmd: Some(command),
                env: Some(
                    environment
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect(),
                ),
                working_dir: cwd.clone(),
                user: user.map(str::to_owned),
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await
        .context("create native supervisor exec")?;
    if let Some(callback) = callback {
        callback.record(exec.id.clone()).await;
    }
    let started = docker
        .start_exec(&exec.id, None::<StartExecOptions>)
        .await
        .context("start native supervisor exec")?;
    let StartExecResults::Attached { mut output, input } = started else {
        anyhow::bail!("native supervisor transport is detached");
    };
    let writer = CommandTask::new_stdin(tokio::spawn(writer(input, receiver, controller.clone())));
    let mut stdout = CappedOutput::new(cap.unwrap_or(usize::MAX));
    let mut stderr = CappedOutput::new(cap.unwrap_or(usize::MAX));
    let mut stderr_tail = Vec::new();
    let mut stdout_filter = redaction.stream();
    let mut stderr_filter = redaction.stream();
    let mut sequencer = ChunkSequencer::new();
    let mut frame = Vec::new();
    let mut terminal = None;
    let mut root_exit = None;
    let mut exec_failed = false;
    let mut terminal_status_expected = false;

    while let Some(chunk) = output.next().await {
        let message = match chunk.context("native supervisor stream failed")? {
            bollard::container::LogOutput::StdOut { message } => message,
            bollard::container::LogOutput::StdErr { message } if message.is_empty() => continue,
            _ => anyhow::bail!("native supervisor emitted unframed control data"),
        };
        for byte in message {
            if byte != b'\n' {
                ensure!(
                    frame.len() < protocol::MAX_FRAME_BYTES,
                    "native supervisor response exceeds its frame bound"
                );
                frame.push(byte);
                continue;
            }
            let event = protocol::decode_event(&frame)?.into_wire();
            if matches!(event, EventWire::Output { .. }) {
                ensure!(
                    frame.len() <= 64 * 1024,
                    "native supervisor output frame exceeds its data bound"
                );
            }
            frame.clear();
            controller.same_id(event_id(&event).context("unexpected native supervisor probe response")?)?;
            if terminal.is_some() {
                match event {
                    EventWire::SignalResult {
                        request_id,
                        signal,
                        delivered: Delivery::NotDelivered,
                        ..
                    } if !terminal_status_expected
                        && controller.state.borrow().pending_signal == Some((request_id, signal))
                        && controller.state.borrow().signal_result.is_none() =>
                    {
                        controller.state.send_modify(|state| {
                            state.pending_signal = None;
                            state.signal_result = Some((request_id, signal, false));
                        });
                        terminal_status_expected = true;
                    }
                    EventWire::Status {
                        phase: protocol::Phase::Absent,
                        ..
                    } if terminal_status_expected => {
                        terminal_status_expected = false;
                    }
                    _ => anyhow::bail!("native supervisor emitted invalid data after its absence proof"),
                }
                continue;
            }
            match event {
                EventWire::Hello {
                    version, environment, ..
                } => {
                    ensure!(
                        version == protocol::VERSION && !controller.state.borrow().hello,
                        "invalid native supervisor greeting"
                    );
                    let mut environment = environment;
                    environment.extend(
                        request
                            .env
                            .as_map()
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone())),
                    );
                    environment.insert(CommandEnv::RESERVED_KEY.into(), controller.id.as_str().into());
                    controller.state.send_modify(|state| state.hello = true);
                    let timeout_ms = match request_deadline(&request) {
                        CommandDeadline::Lease => None,
                        CommandDeadline::Timeout => Some(u64::from(request.timeout_seconds) * 1000),
                    };
                    controller
                        .send(CommandWire::Init {
                            version: protocol::VERSION,
                            execution_id: controller.id.clone(),
                            argv: request.command.clone(),
                            env: environment,
                            cwd: cwd.clone(),
                            timeout_ms,
                            release_timeout_ms: Some(protocol::DEFAULT_RELEASE_TIMEOUT_MS),
                        })
                        .await?;
                }
                EventWire::Ready {
                    version,
                    helper,
                    root,
                    process_group,
                    session,
                    ..
                } => {
                    ensure!(
                        version == protocol::VERSION
                            && helper.pid > 1
                            && root.pid > 1
                            && helper.pid != root.pid
                            && helper.start_time > 0
                            && root.start_time > 0
                            && process_group == i32::try_from(root.pid)?
                            && session == process_group,
                        "invalid native supervisor root binding"
                    );
                    ensure!(
                        controller.state.borrow().hello && controller.state.borrow().ready.is_none(),
                        "native supervisor readiness was reordered"
                    );
                    controller.state.send_modify(|state| {
                        state.ready = Some(Ready {
                            helper,
                            root,
                            process_group,
                            session,
                        })
                    });
                }
                EventWire::Released { .. } => {
                    ensure!(
                        controller.state.borrow().ready.is_some() && !controller.state.borrow().released,
                        "native supervisor release was reordered"
                    );
                    controller.state.send_modify(|state| state.released = true);
                }
                EventWire::Output { stream, data, .. } => {
                    ensure!(
                        controller.state.borrow().released,
                        "native supervisor released output before durable authorization"
                    );
                    if let Some(tap) = &tap {
                        tap.push(&data);
                    }
                    let mut sink = OutputSink {
                        streaming: &streaming,
                        retention,
                        stdout: &mut stdout,
                        stderr: &mut stderr,
                        stderr_tail: &mut stderr_tail,
                    };
                    match stream {
                        protocol::Stream::Stdout => {
                            sink.accept(
                                &mut sequencer,
                                trogon_sandbox_contract::OutputStream::Stdout,
                                stdout_filter.push(&data),
                            )
                            .await
                        }
                        protocol::Stream::Stderr => {
                            sink.accept(
                                &mut sequencer,
                                trogon_sandbox_contract::OutputStream::Stderr,
                                stderr_filter.push(&data),
                            )
                            .await
                        }
                    }
                }
                EventWire::SignalResult {
                    request_id,
                    signal,
                    delivered,
                    ..
                } => {
                    ensure!(
                        controller.state.borrow().pending_signal == Some((request_id, signal)),
                        "native supervisor signal acknowledgement changed"
                    );
                    controller.state.send_modify(|state| {
                        state.pending_signal = None;
                        state.signal_result = Some((request_id, signal, bool::from(delivered)));
                    });
                }
                EventWire::RootExited { exit_code, signal, .. } => {
                    ensure!(
                        (exit_code.is_some() ^ signal.is_some())
                            && exit_code.is_none_or(|code| (0..=255).contains(&code))
                            && signal.is_none_or(|signal| (1..=64).contains(&signal))
                            && root_exit.is_none()
                            && controller.state.borrow().ready.is_some(),
                        "native root exit evidence is malformed"
                    );
                    root_exit = Some((exit_code, signal));
                }
                EventWire::FamilyAbsent {
                    released,
                    root_exit_code,
                    root_signal,
                    timed_out,
                    canceled,
                    reaped_count,
                    stdout_truncated,
                    stderr_truncated,
                    ..
                } => {
                    let (released, canceled) = (bool::from(released), bool::from(canceled));
                    ensure!(
                        controller.state.borrow().ready.is_some()
                            && reaped_count > 0
                            && ((released
                                && (root_exit_code.is_some() ^ root_signal.is_some())
                                && root_exit == Some((root_exit_code, root_signal)))
                                || (!released
                                    && root_exit_code.is_none()
                                    && root_signal.is_none()
                                    && root_exit.is_none()
                                    && canceled))
                            && root_exit_code.is_none_or(|code| (0..=255).contains(&code))
                            && root_signal.is_none_or(|signal| (1..=64).contains(&signal))
                            && released == controller.state.borrow().released
                            && (!exec_failed || matches!(root_exit_code, Some(126 | 127)))
                            && (released || canceled),
                        "native family absence evidence is malformed"
                    );
                    let completion = Completion {
                        released,
                        root_exit_code,
                        root_signal,
                        timed_out: bool::from(timed_out),
                        canceled,
                        reaped_count,
                        stdout_truncated: bool::from(stdout_truncated),
                        stderr_truncated: bool::from(stderr_truncated),
                    };
                    controller
                        .state
                        .send_modify(|state| state.completion = Some(completion.clone()));
                    controller
                        .send(CommandWire::Acknowledge {
                            execution_id: controller.id.clone(),
                        })
                        .await?;
                    terminal = Some(completion);
                }
                EventWire::Error {
                    code: protocol::ErrorCode::ExecFailed,
                    uncertain: Certainty::Certain,
                    ..
                } => {
                    ensure!(
                        controller.state.borrow().released && !exec_failed,
                        "native supervisor exec failure was reordered"
                    );
                    exec_failed = true;
                }
                EventWire::Error { .. } => {
                    anyhow::bail!("native supervisor could not establish complete family custody")
                }
                EventWire::Status { .. } => {}
                EventWire::Probe { .. } => anyhow::bail!("unexpected native supervisor probe response"),
            }
        }
    }
    ensure!(frame.is_empty(), "native supervisor stream ended within a frame");
    ensure!(
        !terminal_status_expected,
        "native supervisor omitted its correlated absent status"
    );
    let completion = terminal.context("native supervisor exited without complete family absence")?;
    writer.dispose().await;
    DockerWorkerProvider::wait_exec_exit_code(docker, &exec.id).await?;
    let mut sink = OutputSink {
        streaming: &streaming,
        retention,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stderr_tail: &mut stderr_tail,
    };
    sink.accept(
        &mut sequencer,
        trogon_sandbox_contract::OutputStream::Stdout,
        stdout_filter.finish(),
    )
    .await;
    sink.accept(
        &mut sequencer,
        trogon_sandbox_contract::OutputStream::Stderr,
        stderr_filter.finish(),
    )
    .await;
    let exit_code = if completion.timed_out {
        CommandOutput::TIMED_OUT_EXIT_CODE
    } else {
        completion
            .root_exit_code
            .unwrap_or_else(|| 128 + completion.root_signal.unwrap_or(9))
    };
    if let Some(context) = &streaming {
        DockerWorkerProvider::publish_exit_marker(context, exit_code).await;
    }
    let truncated = OutputTruncation::from(
        stdout.truncation().is_truncated()
            || stderr.truncation().is_truncated()
            || completion.stdout_truncated
            || completion.stderr_truncated,
    );
    let stderr = if retention == OutputRetention::StreamOnly {
        if stderr_tail.len() > STREAM_ONLY_STDERR_TAIL {
            let excess = stderr_tail.len() - STREAM_ONLY_STDERR_TAIL;
            stderr_tail.drain(..excess);
        }
        stderr_tail
    } else {
        stderr.into_bytes()
    };
    Ok(CommandOutput {
        exit_code,
        stdout: stdout.into_bytes(),
        stderr,
        truncated,
        timed_out: CommandTimeoutStatus::from(completion.timed_out),
    })
}
