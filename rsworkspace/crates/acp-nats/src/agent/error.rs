use agent_client_protocol::{Error, ErrorCode};
use tracing::warn;

/// Maps a NATS error to an ACP internal error with warning log.
///
/// This helper standardizes error handling across all agent handlers,
/// ensuring consistent logging and error code mapping.
pub(super) fn log_and_map_nats_error(e: impl std::fmt::Display) -> Error {
    warn!(error = %e, "NATS request failed");
    Error::new(ErrorCode::InternalError.into(), e.to_string())
}
