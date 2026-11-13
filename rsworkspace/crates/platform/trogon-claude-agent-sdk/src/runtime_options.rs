//! Value types for the options that only change the CLI command line (`thinking`, `betas`, `taskBudget`,
//! `permissionPrompts`, ...). Each maps one to one onto an upstream literal.

/// `ThinkingConfig.display`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingDisplay {
    Summarized,
    Omitted,
}

impl ThinkingDisplay {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summarized => "summarized",
            Self::Omitted => "omitted",
        }
    }
}

/// TypeScript `ThinkingConfig` and Python `ThinkingConfig`.
///
/// TypeScript lets `enabled` omit its budget, which asks for adaptive thinking. Python's `budget_tokens` is a
/// required key, so the Python dialect rejects an `enabled` without one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingConfig {
    Adaptive {
        display: Option<ThinkingDisplay>,
    },
    Enabled {
        budget_tokens: Option<u64>,
        display: Option<ThinkingDisplay>,
    },
    Disabled,
}

/// `taskBudget` / `task_budget`: the API-side task budget in tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBudget {
    pub total: u64,
}

/// `SdkBeta`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SdkBeta {
    Context1m,
}

impl SdkBeta {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Context1m => "context-1m-2025-08-07",
        }
    }
}

/// TypeScript `permissionPrompts`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionPrompts {
    Host,
    None,
}

impl PermissionPrompts {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::None => "none",
        }
    }
}

/// `includeHookEvents` / `include_hook_events`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HookEvents {
    #[default]
    Exclude,
    Include,
}

/// TypeScript `allowDangerouslySkipPermissions`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DangerousPermissionSkip {
    #[default]
    Disallow,
    Allow,
}

/// TypeScript `debug`: whether the CLI logs debug output.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DebugOutput {
    #[default]
    Off,
    On,
}

/// `skills`: which discovered skills the session may use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Skills {
    /// `"all"`: every discovered skill.
    All,
    /// An explicit allowlist of skill names.
    Named(Vec<String>),
}
