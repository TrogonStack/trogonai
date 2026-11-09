pub(crate) fn audit_unavailable(error: &trogon_audit::AuditUnavailableError) {
    tracing::warn!(reason = %error.reason(), "audit readiness probe was not acknowledged");
}

pub(crate) fn audit_worker_failed(error: &tokio::task::JoinError) {
    tracing::warn!(%error, "audit readiness worker ended abnormally");
}

pub(crate) fn http_worker_failed(error: &tokio::task::JoinError) {
    tracing::warn!(%error, "HTTP readiness worker ended abnormally");
}

pub(crate) fn accept_failed(error: &std::io::Error) {
    tracing::warn!(%error, "HTTP readiness accept failed");
}
