use trogon_sandbox_egress_proxy::PolicyRejectedError;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("invalid sandbox configuration: {0:#}")]
    Invalid(anyhow::Error),
    #[error("{0}")]
    Unsupported(String),
    #[error("the egress proxy could not start: {0:#}")]
    Proxy(anyhow::Error),
    #[error("the command could not be launched: {0:#}")]
    Launch(anyhow::Error),
    #[error("the sandbox profile cannot be built: {0:#}")]
    Profile(anyhow::Error),
    #[error("interrupted before the command started")]
    Cancelled,
    #[error("the network policy was rejected: {0:?}")]
    PolicyRejectedError(PolicyRejectedError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl RuntimeError {
    /// A launch failure, typed as `Profile` when the cause is a filesystem or
    /// sandbox profile the backend could not express.
    #[cfg_attr(windows, allow(dead_code))]
    pub(crate) fn launch(error: anyhow::Error) -> Self {
        if error.chain().any(Self::is_profile_cause) {
            Self::Profile(error)
        } else {
            Self::Launch(error)
        }
    }

    #[cfg_attr(windows, allow(dead_code))]
    fn is_profile_cause(cause: &(dyn std::error::Error + 'static)) -> bool {
        #[cfg(target_os = "linux")]
        if cause.is::<trogon_sandbox_subprocess_worker::isolation::mount_plan::FsPlanError>() {
            return true;
        }
        #[cfg(target_os = "macos")]
        if cause.is::<trogon_sandbox_subprocess_worker::isolation::seatbelt::sbpl::SbplError>() {
            return true;
        }
        let _ = cause;
        false
    }

    #[cfg(unix)]
    pub(crate) fn unsupported(reason: &str) -> Self {
        Self::Unsupported(reason.to_string())
    }
}

#[cfg(test)]
mod tests;
