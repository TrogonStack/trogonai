use crate::peer_port_error::PeerPortError;
use trogon_peer::{BatchId, MessageId};
pub trait MessageIdGenerator: Send + Sync {
    fn message_id(&self) -> Result<MessageId, PeerPortError>;
}
pub trait BatchIdGenerator: Send + Sync {
    fn batch_id(&self) -> Result<BatchId, PeerPortError>;
}
