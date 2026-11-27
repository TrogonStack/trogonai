use super::Exit;

/// Why a launch did not end with a container that stayed up.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchGuardError {
    #[error("sandbox container is not running right after start: {reason}")]
    NotRunning { reason: String },
    #[error(transparent)]
    Launch(Box<dyn std::error::Error + Send + Sync>),
}

impl LaunchGuardError {
    pub(super) fn launch<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self::Launch(Box::new(error))
    }

    pub(super) fn not_running(exit: &Exit) -> Self {
        Self::NotRunning {
            reason: exit.reason().to_string(),
        }
    }
}
