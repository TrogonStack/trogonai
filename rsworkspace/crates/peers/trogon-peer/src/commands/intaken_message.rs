use super::domain::{MessageId, TrustedContent};
use chrono::{DateTime, Utc};
use trogonai_proto::peers::v1alpha1 as proto;

#[derive(Debug, Clone)]
pub struct IntakenMessage {
    pub(super) entry: proto::InboxEntry,
    pub(super) metadata: proto::MessageMetadata,
    pub(super) message_id: MessageId,
    pub(super) content: TrustedContent,
    pub(super) accepted_at: DateTime<Utc>,
    pub(super) expires_at: DateTime<Utc>,
}

impl IntakenMessage {
    pub fn entry(&self) -> &proto::InboxEntry {
        &self.entry
    }

    pub fn metadata(&self) -> &proto::MessageMetadata {
        &self.metadata
    }

    pub fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    pub fn content(&self) -> &TrustedContent {
        &self.content
    }

    pub fn accepted_at(&self) -> DateTime<Utc> {
        self.accepted_at
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }
}
