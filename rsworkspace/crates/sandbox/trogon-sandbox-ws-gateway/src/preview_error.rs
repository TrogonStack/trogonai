use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use trogon_sandbox_client::ClientError;

use crate::SessionStoreError;
use crate::deny::deny;

/// Why the preview proxy refused or failed a request.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreviewError {
    #[error("preview access is not enabled")]
    Disabled,
    #[error("the preview token expired")]
    Expired,
    #[error("the preview token is not valid")]
    InvalidToken,
    #[error("preview grant lookup failed: {0}")]
    GrantStore(#[from] SessionStoreError),
    #[error("websocket upgrades are not proxied")]
    UpgradeUnsupported,
    #[error("the upstream is not ready")]
    UpstreamNotReady,
    #[error("the port is not exposed")]
    PortNotExposed,
    #[error("the provider does not support previews")]
    Unsupported,
    #[error("the lease may not be previewed")]
    Denied,
    #[error("preview upstream resolution failed: {0}")]
    Resolution(#[source] ClientError),
    #[error("preview upstream request failed: {0}")]
    Unreachable(#[source] reqwest::Error),
}

impl PreviewError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Disabled => (StatusCode::NOT_FOUND, "preview_disabled"),
            Self::Expired => (StatusCode::UNAUTHORIZED, "preview_token_expired"),
            Self::InvalidToken => (StatusCode::UNAUTHORIZED, "invalid_preview_token"),
            Self::GrantStore(_) => (StatusCode::INTERNAL_SERVER_ERROR, "session_store_failure"),
            Self::UpgradeUnsupported => (StatusCode::NOT_IMPLEMENTED, "preview_upgrade_unsupported"),
            Self::UpstreamNotReady => (StatusCode::BAD_GATEWAY, "upstream_not_ready"),
            Self::PortNotExposed => (StatusCode::NOT_FOUND, "port_not_exposed"),
            Self::Unsupported => (StatusCode::NOT_IMPLEMENTED, "preview_unsupported"),
            Self::Denied => (StatusCode::FORBIDDEN, "preview_denied"),
            Self::Resolution(_) => (StatusCode::BAD_GATEWAY, "upstream_resolution_failed"),
            Self::Unreachable(_) => (StatusCode::BAD_GATEWAY, "upstream_unreachable"),
        }
    }
}

impl IntoResponse for PreviewError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let logged: Option<&dyn std::error::Error> = std::error::Error::source(&self).is_some().then_some(&self);
        deny(status, code, logged)
    }
}
