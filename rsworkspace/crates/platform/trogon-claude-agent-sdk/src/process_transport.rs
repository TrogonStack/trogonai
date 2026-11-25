//! The subprocess implementation of [`Transport`], Python's `SubprocessCLITransport`.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{FutureExt, future::OptionFuture};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand},
    task::JoinHandle,
    time::{sleep, timeout},
};

use crate::{
    ArgvStyle, ClaudeAgentOptions, Error, JsonObject, StderrTail, Transport,
    adapters::SpawnProcess,
    argv,
    constants::{
        CLIENT_APP_ENV, DERIVED_CLIENT_APP_ENV, PYTHON_CLOSE_WAIT, STDERR_SETTLE, TYPESCRIPT_CLOSE_GRACE,
        TYPESCRIPT_KILL_DELAY,
    },
    environment_plan::{self, Inherited},
    framing::{FrameReader, SharedTail, drain_stderr_with},
    process_options::{EnvironmentMode, is_script},
    python_c_stack::PythonCStack,
};

/// Builds the child command. A script path runs under `script_runtime.executable` (default `node`), as the
/// TypeScript SDK does; any other path is the program itself.
pub(crate) fn command(
    options: &ClaudeAgentOptions,
    cli_path: &Path,
    inherited: &Inherited,
    derived_debug_file: Option<&Path>,
    python_stack: PythonCStack,
) -> Result<ProcessCommand, Error> {
    let arguments = argv::build(options, derived_debug_file, python_stack)?;
    let mut command = if is_script(cli_path, options.argv_style) {
        let runtime = options
            .script_runtime
            .executable
            .as_deref()
            .unwrap_or_else(|| Path::new("node"));
        let mut command = ProcessCommand::new(runtime);
        command.args(&options.script_runtime.arguments);
        command.arg(cli_path);
        command
    } else {
        let mut command = ProcessCommand::new(cli_path);
        // TypeScript puts `executableArgs` before the arguments of a native binary too.
        if options.argv_style == ArgvStyle::TypeScript {
            command.args(&options.script_runtime.arguments);
        }
        command
    };
    command.args(arguments);
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    let mut plan = environment_plan::plan(options, inherited);
    let explicit_label = options.client_app.is_some()
        || inherited.contains_key(OsStr::new(CLIENT_APP_ENV))
        || options.env.iter().any(|(key, _)| key == CLIENT_APP_ENV);
    let derived = (options.environment_mode == EnvironmentMode::Overlay && !explicit_label)
        .then(|| crate::client_app_derivation::derive(&options.client_app_derivation, cli_path))
        .flatten();
    if let Some(label) = derived {
        plan.remove.remove(OsStr::new(DERIVED_CLIENT_APP_ENV));
        plan.set.insert(DERIVED_CLIENT_APP_ENV.into(), label.into());
    }
    if plan.clear {
        command.env_clear();
    }
    for name in &plan.remove {
        command.env_remove(name);
    }
    command.envs(&plan.set);
    if options.file_checkpointing == crate::FileCheckpointing::Enabled && options.session_store.is_some() {
        return Err(Error::FileCheckpointingWithSessionStore);
    }
    #[cfg(unix)]
    {
        if crate::inherited_descriptors::passes_only_stdio(options.argv_style) {
            let limit = crate::inherited_descriptors::descriptor_limit();
            // SAFETY: the closure only calls `close_range` and `fcntl`, which are async-signal-safe and allocate
            // nothing.
            unsafe {
                command.pre_exec(move || crate::inherited_descriptors::keep_only_stdio(limit));
            }
        }
        let uid = match (&options.user, options.run_as_uid) {
            (Some(name), _) => Some(crate::run_as::lookup(name).map_err(Error::Io)?),
            (None, uid) => uid,
        };
        if let Some(uid) = uid {
            // `setreuid(-1, -1)` changes nothing yet succeeds, so a privileged parent would keep its identity;
            // CPython refuses that value for `Popen(user=)` too.
            if uid == u32::MAX {
                return Err(Error::Io(std::io::Error::from(std::io::ErrorKind::InvalidInput)));
            }
            // SAFETY: the closure only calls `setreuid`, which is async-signal-safe and allocates nothing.
            unsafe {
                command.pre_exec(move || crate::run_as::set_real_and_effective_uid(uid));
            }
        }
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(command)
}

/// Why the process can no longer be written to, shared with every `ClientHandle` once the driver has stopped.
#[derive(Clone, Default)]
pub(crate) struct Closed(Arc<Mutex<Option<ClosedReason>>>);

#[derive(Clone)]
enum ClosedReason {
    /// Python: `returncode` is set while `_ready` is still true.
    Terminated { exit_code: Option<i32> },
    /// TypeScript: the `exit` event sets `ready = false` before any later write.
    NotReady,
}

impl ClosedReason {
    /// How each SDK words a write once the process has stopped.
    fn after_stop(style: ArgvStyle, exit_code: Option<i32>) -> Self {
        match style {
            ArgvStyle::Python => Self::Terminated { exit_code },
            _ => Self::NotReady,
        }
    }

    fn error(self) -> Error {
        match self {
            Self::Terminated { exit_code } => Error::WriteToTerminatedProcess { exit_code },
            Self::NotReady => Error::NotReadyForWriting,
        }
    }
}

impl Closed {
    fn record(&self, reason: ClosedReason) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(reason);
        }
    }

    /// The write error for a stopped process, or `None` when the run did not record how it ended.
    pub(crate) fn write_error(&self) -> Option<Error> {
        self.0.lock().ok()?.clone().map(ClosedReason::error)
    }
}

pub(crate) struct ProcessSpawn<'a, P> {
    pub options: &'a ClaudeAgentOptions,
    pub cli_path: PathBuf,
    pub inherited: &'a Inherited,
    pub process: &'a P,
    pub config_dir: Option<&'a Path>,
    /// The TypeScript default debug file, derived by `connect` when the SDK spawns the CLI itself.
    pub derived_debug_file: Option<&'a Path>,
    /// `CLAUDE_SECURESTORAGE_CONFIG_DIR` for a child resumed from a store, when the session runtime sets it.
    pub secure_storage_config_dir: Option<std::ffi::OsString>,
    pub closed: Closed,
    /// The caller's whole environment, which a `spawn_claude_code_process` callback receives with the changes
    /// applied. Read only when that callback is set.
    pub parent_environment: Vec<(OsString, OsString)>,
    /// `startup()` and `prewarm()`: ask the CLI to report startup marks, unless the caller's environment already
    /// carries the variable.
    pub startup_marks: bool,
    /// Where the Python SDK builds the command: in `ClaudeSDKClient.connect` or in `query()`.
    pub python_stack: PythonCStack,
}

/// TypeScript's `SpawnOptions` for the command the SDK would run. `signal` is the forwarded abort signal.
fn spawn_options(
    options: &ClaudeAgentOptions,
    inherited: &Inherited,
    parent_environment: Vec<(OsString, OsString)>,
    command: &ProcessCommand,
    signal: crate::CancellationToken,
) -> Result<crate::SpawnOptions, Error> {
    let command = command.as_std();
    let mut env: BTreeMap<OsString, OsString> = if environment_plan::plan(options, inherited).clear {
        BTreeMap::new()
    } else {
        parent_environment.into_iter().collect()
    };
    for (name, value) in command.get_envs() {
        match value {
            Some(value) => env.insert(name.to_owned(), value.to_owned()),
            None => env.remove(name),
        };
    }
    #[cfg(unix)]
    let uid = match &options.user {
        Some(name) => Some(crate::run_as::lookup(name).map_err(Error::Io)?),
        None => options.run_as_uid,
    };
    #[cfg(not(unix))]
    let uid = None;
    Ok(crate::SpawnOptions {
        command: command.get_program().to_owned(),
        args: command.get_args().map(OsStr::to_owned).collect(),
        cwd: command.get_current_dir().map(Path::to_path_buf),
        env,
        signal,
        uid,
    })
}

/// TypeScript logs a throw from `onProcessSpawned` and carries on.
fn report_spawn(callback: &crate::OnProcessSpawned, pid: u32, spawn_sync_ms: f64) {
    let call = std::panic::AssertUnwindSafe(|| (callback.0)(pid, crate::ProcessSpawnInfo { spawn_sync_ms }));
    if std::panic::catch_unwind(call).is_err() {
        tracing::debug!("onProcessSpawned threw");
    }
}

pub(crate) struct ProcessTransport {
    /// `None` once a TypeScript close handed the process to its delayed SIGKILL.
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    frames: FrameReader<ChildStdout>,
    stderr: JoinHandle<Result<(), io::Error>>,
    stderr_tail: SharedTail,
    argv_style: ArgvStyle,
    shutdown: crate::Shutdown,
    closed: Closed,
    /// The process was stopped by the SDK (a kill request or an escalation), so its exit status is not an error.
    forced: bool,
    /// TypeScript's `endInput` ends the stream but keeps the transport ready, so a later write is dropped without
    /// an error (`[ProcessTransport] Dropping write to ended stdin stream`).
    input_ended: bool,
    /// The reader reached the end of stdout, so the CLI finished on its own rather than being stopped.
    stdout_ended: bool,
    /// The `mcp_message` ids of the line `read_message` returned last, for the Python dialect.
    wire_ids: Option<crate::McpWireIds>,
    /// TypeScript's `forwardedAbort` for a `spawn_claude_code_process` process, with the caller's abort controller
    /// it follows once the close grace is over.
    forwarded_abort: Option<ForwardedAbort>,
}

struct ForwardedAbort {
    caller: crate::CancellationToken,
    forwarded: crate::CancellationToken,
}

impl ProcessTransport {
    /// Starts the CLI. Python spawns in `connect`; here the spawn happens first so every spawn failure keeps its
    /// own error before the driver exists, and `connect` has nothing left to do.
    pub(crate) fn spawn<P: SpawnProcess<Error = io::Error>>(input: ProcessSpawn<'_, P>) -> Result<Self, Error> {
        let ProcessSpawn {
            options,
            cli_path,
            inherited,
            process,
            config_dir,
            derived_debug_file,
            secure_storage_config_dir,
            closed,
            parent_environment,
            startup_marks,
            python_stack,
        } = input;
        let mut command = command(options, &cli_path, inherited, derived_debug_file, python_stack)?;
        if startup_marks {
            command.env("CLAUDE_CODE_SDK_READS_STARTUP_MARKS", "1");
        }
        if let Some(config_dir) = config_dir {
            command.env("CLAUDE_CONFIG_DIR", config_dir);
        }
        command.envs(secure_storage_config_dir.map(|directory| ("CLAUDE_SECURESTORAGE_CONFIG_DIR", directory)));
        crate::cli_path::reject_windows_batch_cli(cli_path.as_os_str(), options.windows_batch_cli)?;
        let mut forwarded_abort = None;
        let mut child = if let Some(spawner) = &options.spawn_claude_code_process {
            let forwarded = crate::CancellationToken::new();
            let spawn = spawn_options(options, inherited, parent_environment, &command, forwarded.clone())?;
            forwarded_abort = Some(ForwardedAbort {
                caller: options.abort_controller.clone().unwrap_or_default(),
                forwarded,
            });
            (spawner.0)(spawn).map_err(Error::Spawn)?
        } else {
            let started = std::time::Instant::now();
            let child = process.spawn(&mut command).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    match options.cwd.as_ref() {
                        Some(cwd) if !cwd.exists() => Error::WorkingDirectoryMissing(cwd.clone()),
                        _ => Error::CliNotFoundAt(cli_path.clone()),
                    }
                } else {
                    Error::Spawn(crate::error::launch_failure(&cli_path, error))
                }
            })?;
            let spawn_sync_ms = started.elapsed().as_secs_f64() * 1000.0;
            if let (Some(pid), Some(callback)) = (child.id(), &options.on_process_spawned) {
                report_spawn(callback, pid, spawn_sync_ms);
            }
            child
        };
        let stdin = child.stdin.take().ok_or(Error::PipeUnavailable)?;
        let stdout = child.stdout.take().ok_or(Error::PipeUnavailable)?;
        // A process from `spawn_claude_code_process` may come without a stderr pipe, as in TypeScript. The SDK's own
        // spawn always pipes stderr, like `sdk.mjs`, so a process adapter that drops it fails like a missing pipe.
        let stderr = match child.stderr.take() {
            None if options.spawn_claude_code_process.is_none() => return Err(Error::PipeUnavailable),
            stderr => stderr,
        };
        let stderr_tail = SharedTail::new(options.limits.stderr_tail, options.argv_style == ArgvStyle::TypeScript);
        let stderr = match stderr {
            Some(stderr) => tokio::spawn(drain_stderr_with(
                stderr,
                stderr_tail.clone(),
                options
                    .limits
                    .max_buffer_size
                    .unwrap_or(crate::Limits::PYTHON_MAX_BUFFER_SIZE),
                options.stderr_handler.clone(),
            )),
            None => tokio::spawn(async { Ok(()) }),
        };
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            frames: FrameReader::new(stdout, options.limits.max_buffer_size, options.argv_style),
            stderr,
            stderr_tail,
            argv_style: options.argv_style,
            shutdown: options.limits.shutdown,
            closed,
            forced: false,
            input_ended: false,
            stdout_ended: false,
            wire_ids: None,
            forwarded_abort,
        })
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    /// Why a write cannot go out, in upstream's order: not ready, then terminated. Python keeps `_ready` after the
    /// process exits and reports the terminated process with its exit code. TypeScript's `exit` event clears
    /// `ready`, so a write after an observed exit is "not ready"; its `exited with error` branch is only reachable
    /// while the process is still running with an exit error recorded, a state neither SDK can enter (Python sets
    /// `_exit_error` after `returncode`, TypeScript clears `ready` with the error).
    fn write_unavailable(&mut self) -> Option<Error> {
        if self.stdin.is_none() && !self.input_ended {
            return Some(Error::NotReadyForWriting);
        }
        // One chain: whether the child has exited by the time of a write is a race the tests cannot pin, so the
        // exited and running cases share every line.
        let style = self.argv_style;
        self.child
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten())
            .map(|status| ClosedReason::after_stop(style, crate::error::exit_code(&status)).error())
    }

    /// SIGTERM on Unix, a hard kill where there is no such signal or it cannot be delivered.
    async fn terminate(child: &mut Child) -> Result<(), Error> {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            let delivered = ProcessCommand::new("kill")
                .args(["-TERM", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .is_ok_and(|status| status.success());
            if delivered {
                return Ok(());
            }
        }
        child.start_kill().map_err(Error::Io)
    }

    /// Stops the process once stdin is closed, by the configured [`crate::Shutdown`] policy. `None` means the
    /// SDK stopped the process without waiting for its exit.
    async fn reap(&mut self) -> Result<Option<(ExitStatus, StderrTail)>, Error> {
        match self.shutdown {
            crate::Shutdown::TypeScript => Ok(self.reap_typescript().await),
            crate::Shutdown::Python => Ok(self.reap_python().await),
            crate::Shutdown::Strict(wait) => self.reap_strict(wait.as_duration()).await.map(Some),
        }
    }

    /// TypeScript's close: wait 2 seconds, then SIGTERM (none on Windows), and SIGKILL 5 seconds later if the process
    /// is still running, without waiting for it. Stderr gets 200 ms after the exit. Nothing here fails.
    async fn reap_typescript(&mut self) -> Option<(ExitStatus, StderrTail)> {
        let mut child = self.child.take()?;
        if let Ok(Ok(status)) = timeout(TYPESCRIPT_CLOSE_GRACE, child.wait()).await {
            return Some((status, self.stderr_tail().await));
        }
        self.forced = true;
        #[cfg(unix)]
        if let Err(error) = Self::terminate(&mut child).await {
            tracing::debug!(%error, "could not send SIGTERM to Claude Code");
        }
        // One line, because a test ends long before the delay and the task never resumes.
        tokio::spawn(sleep(TYPESCRIPT_KILL_DELAY).map(move |()| kill_if_running(&mut child)));
        None
    }

    /// Python's close: wait 5 seconds, SIGTERM, wait 5 seconds, SIGKILL, wait 5 seconds, with every failure
    /// suppressed.
    async fn reap_python(&mut self) -> Option<(ExitStatus, StderrTail)> {
        let child = self.child.as_mut()?;
        let mut status = timeout(PYTHON_CLOSE_WAIT, child.wait()).await.ok().and_then(Result::ok);
        if status.is_none() {
            self.forced = true;
            if let Err(error) = Self::terminate(child).await {
                tracing::debug!(%error, "could not terminate Claude Code");
            }
            status = timeout(PYTHON_CLOSE_WAIT, child.wait()).await.ok().and_then(Result::ok);
        }
        if status.is_none() {
            if let Err(error) = child.start_kill() {
                tracing::debug!(%error, "could not kill Claude Code");
            }
            status = timeout(PYTHON_CLOSE_WAIT, child.wait()).await.ok().and_then(Result::ok);
        }
        let status = status?;
        Some((status, self.stderr_tail().await))
    }

    /// Python's ladder with `wait` at each step, failing when the process or its stderr outlives it.
    async fn reap_strict(&mut self, wait: Duration) -> Result<(ExitStatus, StderrTail), Error> {
        let child = self.child.as_mut().ok_or(Error::ShutdownTimeout)?;
        let status = match timeout(wait, child.wait()).await {
            Ok(status) => status.map_err(Error::Io)?,
            Err(_) => {
                self.forced = true;
                Self::terminate(child).await?;
                match timeout(wait, child.wait()).await {
                    Ok(status) => status.map_err(Error::Io)?,
                    Err(_) => {
                        child.start_kill().map_err(Error::Io)?;
                        timeout(wait, child.wait())
                            .await
                            .map_err(|_| Error::ShutdownTimeout)?
                            .map_err(Error::Io)?
                    }
                }
            }
        };
        match timeout(wait, &mut self.stderr).await {
            Ok(result) => result.map_err(Error::DriverTask)?.map_err(Error::Io)?,
            Err(_) => {
                self.stderr.abort();
                return Err(Error::StderrDrainTimeout);
            }
        }
        Ok((status, self.stderr_tail.snapshot()))
    }

    /// TypeScript waits up to 200 ms after the exit for stderr to close, then uses what it has.
    async fn stderr_tail(&mut self) -> StderrTail {
        if let Ok(Ok(Err(error))) = timeout(STDERR_SETTLE, &mut self.stderr).await {
            tracing::debug!(%error, "could not read Claude Code stderr");
        }
        self.stderr_tail.snapshot()
    }
}

impl Transport for ProcessTransport {
    async fn connect(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn write(&mut self, data: &str) -> Result<(), Error> {
        self.write_unavailable().map_or(Ok(()), Err)?;
        if self.input_ended {
            tracing::debug!("dropping a write to an ended stdin stream");
            return Ok(());
        }
        let stdin = self.stdin.as_mut().ok_or(Error::NotReadyForWriting)?;
        stdin.write_all(data.as_bytes()).await.map_err(Error::WriteFailed)
    }

    /// Python strips each line with `str.strip`, skips lines that do not then start with `{`, and fails on a corrupt
    /// `{` line with `CLIJSONDecodeError` except for a corrupt unterminated tail at end of file, which is dropped.
    /// TypeScript skips every line that does not decode, and fails on a `null` line. A line may nest past the
    /// recursion limit of `serde_json`, as the JSON parsers of both SDKs read it: TypeScript's at any depth, and
    /// Python's until `json.loads` raises `RecursionError` near 10,000 levels, which no line skips.
    async fn read_message(&mut self) -> Result<Option<JsonObject>, Error> {
        loop {
            let Some(frame) = self.frames.next().await? else {
                self.stdout_ended = true;
                return Ok(None);
            };
            let python = self.argv_style == ArgvStyle::Python;
            // TypeScript decodes stdout leniently; Python already raised for bytes that are not UTF-8.
            let text = if python {
                std::borrow::Cow::Borrowed(frame.as_slice())
            } else {
                match String::from_utf8_lossy(&frame) {
                    std::borrow::Cow::Borrowed(text) => std::borrow::Cow::Borrowed(text.as_bytes()),
                    std::borrow::Cow::Owned(text) => std::borrow::Cow::Owned(text.into_bytes()),
                }
            };
            if !python && crate::deep_json::javascript_null(&text) {
                return Err(Error::NullMessage("Cannot read properties of null (reading 'type')"));
            }
            let line = if python {
                crate::deep_json::python_strip(&text)
            } else {
                text.trim_ascii_start()
            };
            if !line.starts_with(b"{") {
                if !line.trim_ascii().is_empty() {
                    tracing::debug!("skipping non-JSON line from CLI stdout");
                }
                continue;
            }
            if python {
                match crate::deep_json::python_raise(line) {
                    Some(crate::deep_json::PythonRaise::Recursion(text)) => return Err(Error::Recursion(text)),
                    Some(crate::deep_json::PythonRaise::IntDigits(digits)) => {
                        return Err(Error::IntDigits(crate::python_re::int_parse_limit_text(digits)));
                    }
                    None => {}
                }
            }
            // Python reads the ids of an `mcp_message` with `json.loads`; the scan keeps their exact text and
            // hands the parser the line with those ids replaced by `null`.
            let scan = if python {
                crate::McpWireScan::new(line)
            } else {
                crate::McpWireScan::new(&[])
            };
            let parsed = if python {
                crate::deep_json::parse_object(&crate::deep_json::python_integer_zero(scan.frame(line)))
            } else {
                crate::deep_json::parse_object(scan.frame(&text))
            };
            match parsed {
                Ok(mut object) => {
                    if !python {
                        crate::deep_json::javascript_object(&mut object);
                    }
                    self.wire_ids = scan.into_ids();
                    return Ok(Some(object));
                }
                Err(error) => {
                    if !python || self.frames.last_was_tail() {
                        tracing::debug!("skipping undecodable line from CLI stdout");
                        continue;
                    }
                    return Err(crate::JsonDecodeError::new(line, error).into());
                }
            }
        }
    }

    fn take_wire_ids(&mut self) -> Option<crate::McpWireIds> {
        self.wire_ids.take()
    }

    /// Closes stdin, then waits for the process with the escalation described on `reap`. An unsuccessful exit the
    /// SDK did not cause is `Error::Process`.
    async fn close(&mut self) -> Result<(), Error> {
        self.stdin.take();
        self.input_ended = false;
        let reaped = self.reap().await;
        self.stderr.abort();
        // TypeScript aborts the forwarded signal after the close grace, and only when the caller aborted.
        if let Some(abort) = &self.forwarded_abort
            && abort.caller.is_cancelled()
        {
            abort.forwarded.cancel();
        }
        let Some((status, stderr)) = reaped? else {
            self.closed.record(ClosedReason::after_stop(self.argv_style, None));
            return Ok(());
        };
        let exit_code = crate::error::exit_code(&status);
        // Python's clean end of the stream closes the transport (`_ready` is false) before a later write, which
        // then reports "not ready"; a failed exit or a stopped run leaves it ready and reports the terminated process.
        let reason = if self.argv_style == ArgvStyle::Python && self.stdout_ended && !self.forced && status.success() {
            ClosedReason::NotReady
        } else {
            ClosedReason::after_stop(self.argv_style, exit_code)
        };
        self.closed.record(reason);
        if !self.forced && !status.success() {
            return Err(Error::Process { status, stderr });
        }
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.stdin.is_some() || self.input_ended
    }

    async fn end_input(&mut self) -> Result<(), Error> {
        if self.stdin.take().is_some() && self.argv_style != ArgvStyle::Python {
            self.input_ended = true;
        }
        Ok(())
    }

    async fn kill(&mut self) -> Result<(), Error> {
        self.forced = true;
        // One chain: whether the child exits before the kill lands is a race the tests cannot pin, so the running
        // and exited cases share every line.
        let running = self
            .child
            .as_mut()
            .map(|child| child.try_wait().map(|status| status.is_none().then_some(child)))
            .transpose()
            .map_err(Error::Io)?
            .flatten();
        OptionFuture::from(running.map(Self::terminate)).await.unwrap_or(Ok(()))
    }
}

/// The delayed half of TypeScript's close: SIGKILL only a process that has not exited (`exitCode === null`
/// upstream, where an exited child has no handle left and the kill does nothing). Whether the signal was sent.
fn kill_if_running(child: &mut Child) -> bool {
    let running = matches!(child.try_wait(), Ok(None));
    running && child.start_kill().is_ok()
}

#[cfg(test)]
mod command_tests;

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, unix))]
mod run_as_tests;
