use std::io::{BufRead, BufReader, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use crate::constants::DISPATCH_ARG1;
use crate::refusal::refusal_beside_others;
use crate::run_output::RunOutput;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

/// Keeps the helper from opening a console window of its own.
#[cfg(windows)]
fn without_window(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn without_window(_command: &mut Command) {}

fn spawn_failed(subcommand: &str, error: &std::io::Error) -> WindowsSandboxError {
    WindowsSandboxError::for_subcommand(
        Code::SpawnFailed,
        format!("fence-helper {subcommand}: spawn failed: {error}"),
        subcommand,
    )
}

const POLL: Duration = Duration::from_millis(20);

enum Chunk {
    Stdout(String),
    StderrLine(String),
}

/// An `fence-helper` executable to spawn. The path must be explicit: the packaged
/// executable may sit inside a working-tree write grant, where a planted
/// copy would run as the broker.
#[derive(Debug, Clone)]
pub struct FenceHelper {
    exe: PathBuf,
}

impl FenceHelper {
    pub fn new(exe: impl Into<PathBuf>) -> Result<Self, WindowsSandboxError> {
        let exe = exe.into();
        if !exe.exists() {
            return Err(WindowsSandboxError::new(
                Code::FenceHelperNotFound,
                format!(
                    "the fence-helper path is set to '{}' but the file does not exist",
                    exe.display()
                ),
            ));
        }
        Ok(Self { exe })
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// The command that starts `fence-helper` with `args`, for a caller that owns
    /// the child: a launch that lives as long as the lease it serves.
    pub fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(&self.exe);
        command.arg(DISPATCH_ARG1).args(args);
        without_window(&mut command);
        command
    }

    pub fn run(&self, args: &[String], timeout: Duration) -> Result<RunOutput, WindowsSandboxError> {
        self.run_streaming(args, timeout, &mut |_| {})
    }

    /// As [`FenceHelper::run`], handing each stderr line to `on_line` as it
    /// arrives.
    pub fn run_streaming(
        &self,
        args: &[String],
        timeout: Duration,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<RunOutput, WindowsSandboxError> {
        self.run_with_input(args, None, timeout, on_line)
    }

    /// As [`FenceHelper::run`], with `stdin` written to the child and closed.
    pub fn run_with_stdin(
        &self,
        args: &[String],
        stdin: &str,
        timeout: Duration,
    ) -> Result<RunOutput, WindowsSandboxError> {
        self.run_with_input(args, Some(stdin), timeout, &mut |_| {})
    }

    fn run_with_input(
        &self,
        args: &[String],
        stdin: Option<&str>,
        timeout: Duration,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<RunOutput, WindowsSandboxError> {
        let subcommand = args.first().cloned().unwrap_or_default();
        let mut command = Command::new(&self.exe);
        command
            .arg(DISPATCH_ARG1)
            .args(args)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        without_window(&mut command);
        let mut child = command.spawn().map_err(|e| spawn_failed(&subcommand, &e))?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let bytes = text.as_bytes().to_vec();
            thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            });
        }
        let (tx, rx) = mpsc::channel::<Chunk>();
        let _ = child.stdout.take().map(|mut out| {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = out.read_to_end(&mut bytes);
                let _ = tx.send(Chunk::Stdout(String::from_utf8_lossy(&bytes).into_owned()));
            })
        });
        let _ = child.stderr.take().map(|err| {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(err);
                let mut line = Vec::new();
                while matches!(reader.read_until(b'\n', &mut line), Ok(n) if n > 0) {
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim_end_matches(['\n', '\r']).to_string();
                    line.clear();
                    if tx.send(Chunk::StderrLine(text)).is_err() {
                        return;
                    }
                }
            })
        });
        drop(tx);

        let deadline = (!timeout.is_zero()).then(|| Instant::now() + timeout);
        let expired = || deadline.is_some_and(|at| Instant::now() >= at);
        let timed_out = || {
            WindowsSandboxError::for_subcommand(
                Code::FenceHelperTimeout,
                format!(
                    "fence-helper {} timed out after {}ms",
                    args.join(" "),
                    timeout.as_millis()
                ),
                subcommand.clone(),
            )
        };
        let mut stdout = String::new();
        let mut stderr_lines: Vec<String> = Vec::new();
        loop {
            match rx.recv_timeout(POLL) {
                Ok(Chunk::Stdout(s)) => stdout = s,
                Ok(Chunk::StderrLine(l)) => {
                    on_line(&l);
                    stderr_lines.push(l);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if expired() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(timed_out());
            }
        }
        let status = loop {
            match child.try_wait().map_err(|e| spawn_failed(&subcommand, &e))? {
                Some(status) => break status,
                None if !expired() => thread::sleep(POLL),
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(timed_out());
                }
            }
        };
        let status = status.code();
        if let Some(refusal) = refusal_beside_others(args, status, &stdout) {
            return Err(refusal);
        }
        Ok(RunOutput {
            status,
            stdout: stdout.trim().to_string(),
            stderr: stderr_lines.join("\n").trim().to_string(),
        })
    }

    pub(crate) fn run_json(&self, args: &[String], timeout: Duration) -> Result<String, WindowsSandboxError> {
        let out = self.run(args, timeout)?;
        if out.status != Some(0) {
            return Err(WindowsSandboxError::for_subcommand(
                Code::FenceHelperNonzero,
                format!(
                    "fence-helper {} exited {}: {}",
                    args.join(" "),
                    out.status.map_or_else(|| "null".to_string(), |s| s.to_string()),
                    if out.stderr.is_empty() {
                        &out.stdout
                    } else {
                        &out.stderr
                    }
                ),
                args.first().cloned().unwrap_or_default(),
            ));
        }
        Ok(out.stdout)
    }
}
