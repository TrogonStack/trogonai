use std::any::type_name;

use tracing::warn;
use trogon_sandbox_contract::LeaseError;

/// What a route does with a request body that does not decode.
pub(crate) enum OnDecodeError<Resp> {
    /// Answers with this denial, carrying the decode error as a backend error.
    Reply(fn(LeaseError) -> Resp),
    /// Logs the failure and sends nothing back.
    Warn,
    /// Drops the request without a word.
    Ignore,
}

impl<Resp> OnDecodeError<Resp> {
    /// The reply owed for a `Req` body that failed with `error`, if any.
    pub(crate) fn response<Req>(&self, error: &serde_json::Error) -> Option<Resp> {
        match self {
            Self::Reply(denied) => Some(denied(LeaseError::Backend {
                message: format!("decode error: {error}"),
            })),
            Self::Warn => {
                warn!(error = %error, request = type_name::<Req>(), "failed to decode request");
                None
            }
            Self::Ignore => None,
        }
    }
}

#[cfg(test)]
mod tests;
