use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct MessageMetadataProjection {
    pub(crate) message_id: String,
    pub(crate) sender_peer_id: String,
    pub(crate) recipient_peer_id: String,
    pub(crate) accepted_at: String,
    pub(crate) expires_at: String,
}
