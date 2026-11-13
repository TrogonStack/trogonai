use std::{ffi::OsString, path::PathBuf, sync::Arc};

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
