use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::constants::SCRIPT_EXTENSIONS;

/// How the CLI argument list is spelled. The CLI accepts both spellings for every flag.
///
/// `TypeScript` reproduces `@anthropic-ai/claude-agent-sdk` (`--flag=value`, system prompt delivered in the
/// initialize request). `Python` reproduces `claude-agent-sdk` (`--flag value`, system prompt on the command line).
/// `TypeScript` is the default because the CLI accepts the Python spelling as well, so neither choice changes
/// what the CLI does.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ArgvStyle {
    #[default]
    TypeScript,
    Python,
}

/// TypeScript's `endsWith` test is case-sensitive and works on the whole path string; Python has no script
/// runtime, so its dialect keeps the lenient extension check.
pub(crate) fn is_script(path: &Path, style: ArgvStyle) -> bool {
    match style {
        ArgvStyle::TypeScript => {
            let path = path.to_string_lossy();
            SCRIPT_EXTENSIONS
                .iter()
                .any(|extension| path.ends_with(&format!(".{extension}")))
        }
        ArgvStyle::Python => path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| SCRIPT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())),
    }
}

/// How the child environment is assembled.
///
/// The two upstream SDKs disagree: TypeScript replaces the inherited environment with `options.env` when it is
/// given, Python merges `options.env` over it. Both are exposed and TypeScript is the default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EnvironmentMode {
    /// TypeScript: `Replace` when `ClaudeAgentOptions::env` has entries, otherwise `Inherit`. TypeScript tells an
    /// explicit empty `options.env` apart from an absent one; choose `Replace` for the former.
    #[default]
    TypeScript,
    /// Python: inherited environment, then SDK defaults, then `ClaudeAgentOptions::env` on top.
    Overlay,
    /// TypeScript with `options.env`: the child sees only `ClaudeAgentOptions::env` plus SDK defaults.
    Replace,
    /// TypeScript without `options.env`: the inherited environment plus SDK defaults.
    Inherit,
}

impl EnvironmentMode {
    /// The concrete mode for a configured environment; only `TypeScript` depends on it.
    #[must_use]
    pub fn resolve(self, env: &crate::Environment) -> Self {
        match self {
            Self::TypeScript if env.is_empty() => Self::Inherit,
            Self::TypeScript => Self::Replace,
            mode => mode,
        }
    }
}

/// What to do with an inherited `CLAUDECODE` variable.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClaudeCodeMarker {
    /// Python removes it (`Overlay`); TypeScript leaves it alone (`Replace`, `Inherit`).
    #[default]
    ModeDefault,
    Remove,
    Keep,
}

/// W3C trace context forwarded as `TRACEPARENT` and `TRACESTATE`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceContext {
    pub traceparent: String,
    pub tracestate: Option<String>,
}

/// Delivery granularity of subprocess stderr to a [`StderrHandler`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StderrDelivery {
    /// Python: whole lines with trailing whitespace trimmed and empty lines skipped.
    #[default]
    Lines,
    /// TypeScript: each chunk as read.
    Chunks,
}

#[derive(Clone)]
pub struct StderrHandler {
    pub delivery: StderrDelivery,
    pub callback: Arc<dyn Fn(&str) + Send + Sync>,
}

impl std::fmt::Debug for StderrHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StderrHandler")
            .field("delivery", &self.delivery)
            .finish_non_exhaustive()
    }
}

/// TypeScript `SpawnOptions`: what [`SpawnClaudeCodeProcess`] receives.
#[derive(Clone)]
pub struct SpawnOptions {
    /// The program: the CLI itself, or the script runtime when the CLI is a script.
    pub command: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// The whole environment the CLI gets, not only the changes to the caller's.
    pub env: std::collections::BTreeMap<OsString, OsString>,
    /// TypeScript's forwarded abort signal: cancelled when the client closes after the query's abort controller was
    /// cancelled, once the close grace is over, so hanging teardown on it never cuts the graceful shutdown short.
    /// Cancelling it does not abort the query.
    pub signal: crate::CancellationToken,
    /// Python's `user` or `run_as_uid`, resolved to a uid (Unix only). TypeScript has no such option.
    pub uid: Option<u32>,
}

impl std::fmt::Debug for SpawnOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpawnOptions")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("uid", &self.uid)
            .finish_non_exhaustive()
    }
}

/// TypeScript `spawnClaudeCodeProcess`: starts the CLI in place of the SDK. The child must have piped stdin and
/// stdout; its stderr is read when piped. As in TypeScript, the SDK then derives no default debug file and does not
/// call [`OnProcessSpawned`]. A failure is [`crate::Error::Spawn`]. Which descriptors above stderr the child keeps is
/// up to the function: the SDK's own spawn marks them close-on-exec as each dialect's upstream does, and that does not
/// apply here.
#[derive(Clone)]
pub struct SpawnClaudeCodeProcess(
    pub Arc<dyn Fn(SpawnOptions) -> std::io::Result<tokio::process::Child> + Send + Sync>,
);

impl std::fmt::Debug for SpawnClaudeCodeProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SpawnClaudeCodeProcess").finish_non_exhaustive()
    }
}

/// The second argument of TypeScript's `onProcessSpawned`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessSpawnInfo {
    /// `spawnSyncMs`: milliseconds the spawn call itself took.
    pub spawn_sync_ms: f64,
}

/// TypeScript `onProcessSpawned`, a runtime option its declarations omit: called with the process id once the SDK
/// has started the CLI itself. A panic in it is logged and ignored, as TypeScript ignores a throw.
#[derive(Clone)]
pub struct OnProcessSpawned(pub Arc<dyn Fn(u32, ProcessSpawnInfo) + Send + Sync>);

impl std::fmt::Debug for OnProcessSpawned {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("OnProcessSpawned").finish_non_exhaustive()
    }
}

/// One `extraArgs` entry. A `None` value is a bare flag. The flag is passed without the leading dashes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraArgument {
    pub flag: String,
    pub value: Option<String>,
}

/// TypeScript `executable` and `executableArgs`: the runtime that starts a script CLI path.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScriptRuntime {
    pub executable: Option<PathBuf>,
    pub arguments: Vec<OsString>,
}
