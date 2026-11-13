//! The subprocess implementation of [`Transport`], Python's `SubprocessCLITransport`.

use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    ArgvStyle, ClaudeAgentOptions, Error, JsonObject, StderrTail, Transport,
    adapters::SpawnProcess,
    argv,
    constants::{CLIENT_APP_ENV, DERIVED_CLIENT_APP_ENV, SCRIPT_EXTENSIONS},
    environment_plan::{self, Inherited},
    framing::{FrameReader, SharedTail, drain_stderr_with},
    process_options::EnvironmentMode,
};

fn is_script(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| SCRIPT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Builds the child command. A script path runs under `script_runtime.executable` (default `node`), as the
/// TypeScript SDK does; any other path is the program itself.
pub(crate) fn command(
    options: &ClaudeAgentOptions,
    cli_path: &Path,
    inherited: &Inherited,
    derived_debug_file: Option<&Path>,
) -> Result<ProcessCommand, Error> {
    let arguments = argv::build(options, derived_debug_file)?;
    let mut command = if is_script(cli_path) {
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
        ProcessCommand::new(cli_path)
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
    if let Some(name) = &options.user {
        command.uid(crate::run_as::lookup(name).map_err(Error::Io)?);
    } else if let Some(uid) = options.run_as_uid {
        command.uid(uid);
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

impl Closed {
    fn record(&self, reason: ClosedReason) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(reason);
        }
    }

    /// The write error for a stopped process, or `None` when the run did not record how it ended.
    pub(crate) fn write_error(&self) -> Option<Error> {
        let reason = self.0.lock().ok()?.clone()?;
        Some(match reason {
            ClosedReason::Terminated { exit_code } => Error::WriteToTerminatedProcess { exit_code },
            ClosedReason::NotReady => Error::NotReadyForWriting,
        })
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
    pub closed: Closed,
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
            closed,
        } = input;
        let mut command = command(options, &cli_path, inherited, derived_debug_file)?;
        if let Some(config_dir) = config_dir {
            command.env("CLAUDE_CONFIG_DIR", config_dir);
        }
        crate::cli_path::reject_windows_batch_cli(cli_path.as_os_str(), options.windows_batch_cli)?;
        let mut child = process.spawn(&mut command).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                match options.cwd.as_ref() {
                    Some(cwd) if !cwd.exists() => Error::WorkingDirectoryMissing(cwd.clone()),
                    _ => Error::CliNotFoundAt(cli_path.clone()),
                }
            } else {
                Error::Spawn(error)
            }
        })?;
        let stdin = child.stdin.take().ok_or(Error::PipeUnavailable)?;
        let stdout = child.stdout.take().ok_or(Error::PipeUnavailable)?;
        let stderr = child.stderr.take().ok_or(Error::PipeUnavailable)?;
        let stderr_tail = SharedTail::new(options.limits.stderr_tail);
        let stderr = tokio::spawn(drain_stderr_with(
            stderr,
            stderr_tail.clone(),
            options
                .limits
                .max_buffer_size
                .unwrap_or(crate::Limits::PYTHON_MAX_BUFFER_SIZE)
                .get(),
            options.stderr_handler.clone(),
        ));
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            frames: FrameReader::new(stdout, options.limits.max_buffer_size.map(std::num::NonZeroUsize::get)),
            stderr,
            stderr_tail,
            argv_style: options.argv_style,
            shutdown: options.limits.shutdown,
            closed,
            forced: false,
            input_ended: false,
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
        if let Some(Ok(Some(status))) = self.child.as_mut().map(Child::try_wait) {
            return Some(match self.argv_style {
                ArgvStyle::Python => Error::WriteToTerminatedProcess {
                    exit_code: crate::error::exit_code(&status),
                },
                _ => Error::NotReadyForWriting,
            });
        }
        None
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

    /// TypeScript's close: wait 2 seconds, then SIGTERM (none on Windows), and SIGKILL 5 seconds later without
    /// waiting for it. Stderr gets 200 ms after the exit. Nothing here fails.
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
        tokio::spawn(async move {
            if timeout(TYPESCRIPT_KILL_DELAY, child.wait()).await.is_err() {
                if let Err(error) = child.start_kill() {
                    tracing::debug!(%error, "could not send SIGKILL to Claude Code");
                }
                let _ = child.wait().await;
            }
        });
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

/// TypeScript `qet`: the wait for the process to exit on its own after stdin closes.
const TYPESCRIPT_CLOSE_GRACE: Duration = Duration::from_millis(2000);
/// TypeScript: the wait between SIGTERM and SIGKILL.
const TYPESCRIPT_KILL_DELAY: Duration = Duration::from_millis(5000);
/// TypeScript `Yet`: how long stderr may stay open after the exit.
const STDERR_SETTLE: Duration = Duration::from_millis(200);
/// Python: each wait of its close sequence.
const PYTHON_CLOSE_WAIT: Duration = Duration::from_secs(5);

impl Transport for ProcessTransport {
    async fn connect(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn write(&mut self, data: &str) -> Result<(), Error> {
        if let Some(error) = self.write_unavailable() {
            return Err(error);
        }
        if self.input_ended {
            tracing::debug!("dropping a write to an ended stdin stream");
            return Ok(());
        }
        let stdin = self.stdin.as_mut().ok_or(Error::NotReadyForWriting)?;
        stdin.write_all(data.as_bytes()).await.map_err(Error::WriteFailed)
    }

    /// Python skips lines that do not start with `{`, and fails on a corrupt `{` line with `CLIJSONDecodeError`
    /// except for a corrupt unterminated tail at end of file, which is dropped. TypeScript skips every line that
    /// does not decode.
    async fn read_message(&mut self) -> Result<Option<JsonObject>, Error> {
        loop {
            let Some(frame) = self.frames.next().await? else {
                return Ok(None);
            };
            if !frame.trim_ascii_start().starts_with(b"{") {
                if !frame.trim_ascii().is_empty() {
                    tracing::debug!("skipping non-JSON line from CLI stdout");
                }
                continue;
            }
            match serde_json::from_slice::<JsonObject>(&frame) {
                Ok(object) => return Ok(Some(object)),
                Err(error) => {
                    if self.argv_style == ArgvStyle::TypeScript || self.frames.last_was_tail() {
                        tracing::debug!("skipping undecodable line from CLI stdout");
                        continue;
                    }
                    return Err(crate::JsonDecodeError::new(&frame, error).into());
                }
            }
        }
    }

    /// Closes stdin, then waits for the process with the escalation described on `reap`. An unsuccessful exit the
    /// SDK did not cause is `Error::Process`.
    async fn close(&mut self) -> Result<(), Error> {
        self.stdin.take();
        self.input_ended = false;
        let reaped = self.reap().await;
        self.stderr.abort();
        let Some((status, stderr)) = reaped? else {
            self.closed.record(match self.argv_style {
                ArgvStyle::Python => ClosedReason::Terminated { exit_code: None },
                _ => ClosedReason::NotReady,
            });
            return Ok(());
        };
        let exit_code = crate::error::exit_code(&status);
        self.closed.record(match self.argv_style {
            ArgvStyle::Python => ClosedReason::Terminated { exit_code },
            _ => ClosedReason::NotReady,
        });
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
        if let Some(child) = self.child.as_mut()
            && child.try_wait().map_err(Error::Io)?.is_none()
        {
            Self::terminate(child).await?;
        }
        Ok(())
    }
}
