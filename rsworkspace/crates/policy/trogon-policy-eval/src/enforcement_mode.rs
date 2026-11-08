use std::fmt;

/// Whether a refusal blocks the request or is only recorded.
///
/// There is no default and there is no `Off`: absence of policy is denial, so
/// every caller states which of the two it wants. A configuration layer that
/// wants a default should pick [`EnforcementMode::Enforce`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnforcementMode {
    /// A refusal is recorded as `would_deny` and the request is admitted.
    DryRun,
    /// A refusal blocks the request.
    Enforce,
}

impl EnforcementMode {
    /// A stable name for configuration and logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Enforce => "enforce",
        }
    }
}

impl fmt::Display for EnforcementMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
