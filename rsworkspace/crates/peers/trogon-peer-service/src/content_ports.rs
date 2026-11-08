use crate::{
    content_binding::ContentBinding, message_text::MessageText, peer_access::PeerAccess, peer_port_error::PeerPortError,
};
use std::future::Future;
/// Authorize the tenant, seal the text, and return a ClaimCheck that binds stored-byte integrity.
/// The independent semantic fingerprint must use a reviewed keyed construction and remain stable across location and nonce changes.
/// Neither value grants read authority. No production implementation is provided here.
pub trait ContentSeal: Send + Sync {
    fn seal(
        &self,
        scope: &PeerAccess,
        text: &MessageText,
    ) -> impl Future<Output = Result<ContentBinding, PeerPortError>> + Send;
}
/// Before returning text, independently verify the authenticated scope, ClaimCheck stored-byte integrity,
/// exact opened UTF-8 byte size, and stable semantic fingerprint. Neither digest may substitute for the other.
/// `None` means authorized, confirmed absence or erasure. Infrastructure and key-service failures must remain errors.
pub trait ContentOpen: Send + Sync {
    fn open(
        &self,
        scope: &PeerAccess,
        binding: &ContentBinding,
    ) -> impl Future<Output = Result<Option<MessageText>, PeerPortError>> + Send;
}
