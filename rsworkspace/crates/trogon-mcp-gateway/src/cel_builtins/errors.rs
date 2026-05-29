use std::fmt;

/// Classifies host dependency outcomes per `docs/identity/failure-mode-matrix.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailure {
    /// Retryable / unreachable dependency (e.g. SpiceDB gRPC, cluster rate KV).
    Transient,
    /// Caller or policy fault; maps to `-32101` `policy_fault`.
    Permanent,
    /// Not an error for the builtin contract (e.g. cache miss, rate-limited deny).
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CelBuiltinsError {
    NotImplemented(&'static str),
    WrongArity {
        name: &'static str,
        expected: usize,
        got: usize,
    },
    WrongType {
        name: &'static str,
        position: usize,
        expected: &'static str,
    },
    Host {
        name: &'static str,
        failure: HostFailure,
        detail: String,
    },
}

impl CelBuiltinsError {
    #[must_use]
    pub const fn host_failure(&self) -> Option<HostFailure> {
        match self {
            Self::Host { failure, .. } => Some(*failure),
            _ => None,
        }
    }

    pub fn policy_fault(name: &'static str, detail: impl Into<String>) -> Self {
        Self::Host {
            name,
            failure: HostFailure::Permanent,
            detail: detail.into(),
        }
    }

    pub fn authz_unreachable(name: &'static str, detail: impl Into<String>) -> Self {
        Self::Host {
            name,
            failure: HostFailure::Transient,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for CelBuiltinsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented(name) => write!(f, "cel builtin not implemented: {name}"),
            Self::WrongArity { name, expected, got } => {
                write!(f, "{name}: expected {expected} argument(s), got {got}")
            }
            Self::WrongType {
                name,
                position,
                expected,
            } => write!(
                f,
                "{name}: argument {position} has wrong type (expected {expected})"
            ),
            Self::Host {
                name,
                failure,
                detail,
            } => match failure {
                HostFailure::Transient => write!(f, "{name}: authz_unreachable: {detail}"),
                HostFailure::Permanent => write!(f, "{name}: policy_fault: {detail}"),
                HostFailure::NotApplicable => write!(f, "{name}: {detail}"),
            },
        }
    }
}

impl std::error::Error for CelBuiltinsError {}
