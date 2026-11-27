use serde::Serialize;

/// The reply a dispatch that failed to decode gets.
#[derive(Debug, Serialize)]
pub(super) struct DecodeErrorReply {
    pub(super) decode_error: String,
}
