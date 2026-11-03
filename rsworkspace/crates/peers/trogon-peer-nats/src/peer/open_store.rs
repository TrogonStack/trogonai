use async_nats::jetstream;
use trogon_decider_nats::JetStreamStore;

use super::{PeerStoreConfig, PeerStoreError, PeerSubjectResolver};

pub async fn open_store(
    _context: jetstream::Context,
    _config: &PeerStoreConfig,
) -> Result<JetStreamStore<PeerSubjectResolver>, PeerStoreError> {
    Err(PeerStoreError::Unimplemented)
}
