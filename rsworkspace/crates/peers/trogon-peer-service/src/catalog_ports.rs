use crate::{peer_access::PeerAccess, peer_port_error::PeerPortError, work_limit::WorkLimit};
use std::future::Future;
use trogon_peer::{CollaborationRef, PeerId};
/// Trusted projection membership is distinct from attachment liveness and progress.
pub trait PeerCatalog: Send + Sync {
    fn page(
        &self,
        scope: &PeerAccess,
        collaboration: &CollaborationRef,
        after: Option<&PeerId>,
        limit: WorkLimit,
    ) -> impl Future<Output = Result<Vec<PeerId>, PeerPortError>> + Send;
}
