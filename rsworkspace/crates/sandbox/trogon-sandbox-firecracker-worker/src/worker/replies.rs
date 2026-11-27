use serde::Serialize;

/// The reply to a dispatch whose payload could not be decoded or verified.
#[derive(Serialize)]
pub(super) struct DecodeErrorReply<'a> {
    pub(super) decode_error: &'a str,
}

/// The reply to a dispatch whose launch failed.
#[derive(Serialize)]
pub(super) struct ProviderErrorReply<'a> {
    pub(super) provider_error: &'a str,
}
