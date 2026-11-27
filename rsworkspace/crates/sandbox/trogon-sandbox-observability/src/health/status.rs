use axum::http::StatusCode;

/// Answer of `/-/liveness`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    /// The process has not finished its own initialization.
    Starting,
}

/// Why a process is not ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotReady {
    Starting,
    ShuttingDown,
    /// A required dependency has been down for longer than the grace period.
    DependencyDown(&'static str),
}

/// Answer of `/-/readiness`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    NotReady(NotReady),
}

impl From<Liveness> for StatusCode {
    fn from(liveness: Liveness) -> Self {
        match liveness {
            Liveness::Alive => Self::OK,
            Liveness::Starting => Self::SERVICE_UNAVAILABLE,
        }
    }
}

impl From<Readiness> for StatusCode {
    fn from(readiness: Readiness) -> Self {
        match readiness {
            Readiness::Ready => Self::OK,
            Readiness::NotReady(_) => Self::SERVICE_UNAVAILABLE,
        }
    }
}
