use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_contract::{DomainPattern, FsPolicy};

use trogon_sandbox_egress_proxy::CommandId;
use trogon_sandbox_subprocess_worker::attribution::CommandAttribution;

use crate::command_overrides::CommandOverrides;
use crate::constants::DEFAULT_OUTPUT_LIMIT_BYTES;
use trogon_sandbox_subprocess_worker::isolation::{ChildStdio, StdioMode};

/// How much of each captured stream is kept: the first and last halves, with
/// a marker over the dropped middle. The rest is read and dropped so the child
/// never blocks on a full pipe.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutputLimit {
    Bytes(usize),
    Unlimited,
}

impl Default for OutputLimit {
    fn default() -> Self {
        Self::Bytes(DEFAULT_OUTPUT_LIMIT_BYTES)
    }
}

impl OutputLimit {
    pub(crate) fn cap(self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes,
            Self::Unlimited => usize::MAX,
        }
    }
}

/// Which captured streams lost output to the [`OutputLimit`].
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct Truncation {
    pub stdout: bool,
    pub stderr: bool,
}

impl Truncation {
    pub fn any(&self) -> bool {
        self.stdout || self.stderr
    }
}

/// A program to run inside the sandbox.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Captured by default (no input). A stream set to `Inherit` or `Null`
    /// yields no bytes in the output. A piped stdin is closed at once.
    pub stdio: ChildStdio,
    pub output_limit: OutputLimit,
    /// The directory the command starts in. Only the host view can honor it;
    /// the guest view always starts in the workspace.
    pub cwd: Option<PathBuf>,
    /// The name violations are recorded under. A fresh one when unset.
    pub command_id: Option<CommandId>,
    /// The shell command line. Names the command in the
    /// proxy user name when there is no id; the program and arguments do
    /// when it is unset.
    pub command_line: Option<String>,
    /// The command is killed once it has run this long.
    pub timeout: Option<Duration>,
    /// Destinations this command alone may reach, on top of the network
    /// policy. They never lift a denied domain, and a strict allowlist
    /// ignores them.
    pub network_allow: Vec<DomainPattern>,
    /// Replaces the runtime's filesystem policy for this command alone.
    pub fs_policy: Option<FsPolicy>,
    /// Replaces the runtime's network policy for this command alone. Later
    /// policy updates no longer reach it.
    pub network_policy: Option<NetworkPolicy>,
    /// The fields set here replace the session's for
    /// this command alone. A `fs_policy` set as well replaces the result.
    pub overrides: Option<CommandOverrides>,
    /// Stops the command when cancelled.
    pub cancel: Option<CancellationToken>,
}

impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            stdio: ChildStdio::captured(),
            output_limit: OutputLimit::default(),
            cwd: None,
            command_id: None,
            command_line: None,
            timeout: None,
            network_allow: Vec::new(),
            fs_policy: None,
            network_policy: None,
            overrides: None,
            cancel: None,
        }
    }

    /// `/bin/sh -c script`.
    pub fn shell(script: impl Into<String>) -> Self {
        let script = script.into();
        Self::new("/bin/sh").arg("-c").arg(script.clone()).command_line(script)
    }

    pub fn command_line(mut self, command_line: impl Into<String>) -> Self {
        self.command_line = Some(command_line.into());
        self
    }

    /// What names this command to the proxy: the id given by the caller or
    /// else the command line.
    pub(crate) fn attribution(&self) -> CommandAttribution {
        let key = match (&self.command_id, &self.command_line) {
            (Some(id), _) if !id.as_str().is_empty() => id.as_str().to_string(),
            (_, Some(line)) => line.clone(),
            _ => self.display_text(),
        };
        CommandAttribution::new(&key)
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    pub fn stdin(mut self, mode: StdioMode) -> Self {
        self.stdio.stdin = mode;
        self
    }

    pub fn stdout(mut self, mode: StdioMode) -> Self {
        self.stdio.stdout = mode;
        self
    }

    pub fn stderr(mut self, mode: StdioMode) -> Self {
        self.stdio.stderr = mode;
        self
    }

    pub fn stdio(mut self, stdio: ChildStdio) -> Self {
        self.stdio = stdio;
        self
    }

    /// The program and its arguments joined by spaces: the text that
    /// violation-ignore rules keyed by command are matched against.
    pub fn display_text(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn fs_policy(mut self, fs: FsPolicy) -> Self {
        self.fs_policy = Some(fs);
        self
    }

    pub fn network_policy(mut self, network: NetworkPolicy) -> Self {
        self.network_policy = Some(network);
        self
    }

    pub fn overrides(mut self, overrides: CommandOverrides) -> Self {
        self.overrides = Some(overrides);
        self
    }

    pub fn cancel_on(mut self, token: CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Names the command. The id must be at least
    /// [`MIN_COMMAND_ID_LEN`](crate::constants::MIN_COMMAND_ID_LEN) characters so it cannot be guessed.
    pub fn command_id(mut self, id: CommandId) -> Self {
        self.command_id = Some(id);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn network_allow(mut self, domain: DomainPattern) -> Self {
        self.network_allow.push(domain);
        self
    }

    pub fn output_limit(mut self, limit: OutputLimit) -> Self {
        self.output_limit = limit;
        self
    }
}

/// How a command ended and what it wrote. `command` names it in the
/// violation store.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub command: CommandId,
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: Truncation,
    pub timed_out: bool,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status.success()
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

#[cfg(test)]
mod tests;
