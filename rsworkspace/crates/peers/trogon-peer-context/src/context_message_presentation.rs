use buffa_types::google::protobuf::Timestamp;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct ContextMessagePresentation<'a> {
    pub(crate) message_id: &'a str,
    pub(crate) sender_peer_id: &'a str,
    pub(crate) recipient_peer_id: &'a str,
    pub(crate) collaboration_ref: &'a str,
    pub(crate) accepted_at: &'a Timestamp,
    pub(crate) text: &'a str,
}
