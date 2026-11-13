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
    constants::SCRIPT_EXTENSIONS,
    environment_plan::{self, Inherited},
    framing::{FrameReader, drain_stderr_with},
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
) -> Result<ProcessCommand, Error> {
    let arguments = argv::build(options)?;
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
    let plan = environment_plan::plan(options, inherited);
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
    pub closed: Closed,
}

pub(crate) struct ProcessTransport {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: FrameReader<ChildStdout>,
    stderr: JoinHandle<Result<StderrTail, io::Error>>,
    argv_style: ArgvStyle,
    shutdown_timeout: Duration,
    closed: Closed,
    /// The process was stopped by the SDK (a kill request or an escalation), so its exit status is not an error.
    forced: bool,
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
            closed,
        } = input;
        let mut command = command(options, &cli_path, inherited)?;
        if let Some(config_dir) = config_dir {
            command.env("CLAUDE_CONFIG_DIR", config_dir);
        }
        crate::cli_path::reject_windows_batch_cli(cli_path.as_os_str())?;
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
        let stderr = tokio::spawn(drain_stderr_with(
            stderr,
            options.limits.max_stderr_bytes.get(),
            options.stderr_handler.clone(),
        ));
        Ok(Self {
            child,
            stdin: Some(stdin),
            frames: FrameReader::new(stdout, options.limits.max_frame_bytes.get()),
            stderr,
            argv_style: options.argv_style,
            shutdown_timeout: options.limits.shutdown_timeout.as_duration(),
            closed,
            forced: false,
        })
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Why a write cannot go out, in upstream's order: not ready, then terminated. Python keeps `_ready` after the
    /// process exits and reports the terminated process with its exit code. TypeScript's `exit` event clears
    /// `ready`, so a write after an observed exit is "not ready"; its `exited with error` branch is only reachable
    /// while the process is still running with an exit error recorded, a state neither SDK can enter (Python sets
    /// `_exit_error` after `returncode`, TypeScript clears `ready` with the error).
    fn write_unavailable(&mut self) -> Option<Error> {
        if self.stdin.is_none() {
            return Some(Error::NotReadyForWriting);
        }
        if let Ok(Some(status)) = self.child.try_wait() {
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
    async fn terminate(&mut self) -> Result<(), Error> {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
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
        self.child.start_kill().map_err(Error::Io)
    }

    /// Python's close sequence: stdin is already closed, wait, SIGTERM, wait, SIGKILL, wait.
    async fn reap(&mut self) -> Result<(ExitStatus, StderrTail), Error> {
        let wait = self.shutdown_timeout;
        let status = match timeout(wait, self.child.wait()).await {
            Ok(status) => status.map_err(Error::Io)?,
            Err(_) => {
                self.forced = true;
                self.terminate().await?;
                match timeout(wait, self.child.wait()).await {
                    Ok(status) => status.map_err(Error::Io)?,
                    Err(_) => {
                        self.child.start_kill().map_err(Error::Io)?;
                        timeout(wait, self.child.wait())
                            .await
                            .map_err(|_| Error::ShutdownTimeout)?
                            .map_err(Error::Io)?
                    }
                }
            }
        };
        let stderr = match timeout(wait, &mut self.stderr).await {
            Ok(result) => result.map_err(Error::DriverTask)?.map_err(Error::Io)?,
            Err(_) => {
                self.stderr.abort();
                return Err(Error::StderrDrainTimeout);
            }
        };
        Ok((status, stderr))
    }
}

impl Transport for ProcessTransport {
    async fn connect(&mut self) -> Result<(), Error> {
        Ok(())
    }

    async fn write(&mut self, data: &str) -> Result<(), Error> {
        if let Some(error) = self.write_unavailable() {
            return Err(error);
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
        let reaped = self.reap().await;
        self.stderr.abort();
        let (status, stderr) = reaped?;
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
        self.stdin.is_some()
    }

    async fn end_input(&mut self) -> Result<(), Error> {
        self.stdin.take();
        Ok(())
    }

    async fn kill(&mut self) -> Result<(), Error> {
        self.forced = true;
        if self.child.try_wait().map_err(Error::Io)?.is_none() {
            self.terminate().await?;
        }
        Ok(())
    }
}
