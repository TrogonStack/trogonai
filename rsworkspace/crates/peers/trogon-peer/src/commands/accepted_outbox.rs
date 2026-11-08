use super::{AcceptedMessage, domain::IdempotencyKey};

#[derive(Debug, Clone)]
pub(super) struct AcceptedOutbox {
    pub(super) key: IdempotencyKey,
    pub(super) evidence: AcceptedMessage,
}
