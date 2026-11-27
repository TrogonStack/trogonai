use trogon_sandbox_contract::LeaseError;

use crate::api::ApiError;

pub(crate) fn denied(error: &ApiError) -> LeaseError {
    LeaseError::Backend {
        message: error.to_string(),
    }
}

pub(crate) fn invalid_spec(reason: impl Into<String>) -> LeaseError {
    LeaseError::InvalidSpec { reason: reason.into() }
}

/// A failure after the command was launched cannot prove the command did not run.
pub(crate) fn after_launch(error: LeaseError) -> LeaseError {
    match error {
        LeaseError::Backend { .. } => error,
        error => LeaseError::Backend {
            message: format!("execution outcome is unconfirmed: {error}"),
        },
    }
}

pub(crate) fn custody_unavailable() -> LeaseError {
    LeaseError::Backend {
        message: "command launch custody could not establish durable authority".into(),
    }
}
