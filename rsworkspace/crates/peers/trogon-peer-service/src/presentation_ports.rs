use crate::{
    peer_port_error::PeerPortError, prepared_presentation::PreparedPresentation, presentation_key::PresentationKey,
};
use std::future::Future;
pub trait PresentationRead: Send + Sync {
    fn read(
        &self,
        key: &PresentationKey,
    ) -> impl Future<Output = Result<Option<PreparedPresentation>, PeerPortError>> + Send;
}
/// Insert must durably and atomically refuse a different existing record. It must never overwrite.
pub trait PresentationInsert: Send + Sync {
    fn insert(
        &self,
        key: &PresentationKey,
        record: &PreparedPresentation,
    ) -> impl Future<Output = Result<(), PeerPortError>> + Send;
}
/// A cache insertion starts as a candidate. This marks a losing or failed proposer for retention-aware orphan collection.
pub trait PresentationOrphan: Send + Sync {
    fn orphan(&self, key: &PresentationKey) -> impl Future<Output = Result<(), PeerPortError>> + Send;
}
