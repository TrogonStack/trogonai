use crate::{
    content_binding::ContentBinding, message_text::MessageText, peer_access::PeerAccess, peer_port_error::PeerPortError,
};
use std::future::Future;
/// Implementations must authorize the tenant, seal the text, and provide a stable keyed semantic fingerprint independent of location and nonce. No implementation is provided here.
pub trait ContentSeal: Send + Sync {
    fn seal(
        &self,
        scope: &PeerAccess,
        text: &MessageText,
    ) -> impl Future<Output = Result<ContentBinding, PeerPortError>> + Send;
}
/// Authenticate the scoped reference and verify its exact UTF-8 byte size and stable semantic fingerprint before returning text.
/// `None` means authorized, confirmed absence or erasure. Infrastructure and key-service failures must return an error, leaving obligations retryable.
pub trait ContentOpen: Send + Sync {
    fn open(
        &self,
        scope: &PeerAccess,
        binding: &ContentBinding,
    ) -> impl Future<Output = Result<Option<MessageText>, PeerPortError>> + Send;
}
