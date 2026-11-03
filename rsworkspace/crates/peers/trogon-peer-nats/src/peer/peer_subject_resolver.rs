use async_nats::jetstream;
use trogon_decider_nats::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectScope, SubjectScopeError, SubjectState,
};
use trogon_peer::PeerId;

use crate::PeerTenant;

#[derive(Debug, thiserror::Error)]
pub enum PeerSubjectError {
    #[error("peer event subject resolution is not implemented")]
    Unimplemented,
    #[error("peer event scope is invalid: {0}")]
    Scope(#[from] SubjectScopeError),
    #[error("peer event subject is invalid: {0}")]
    Subject(#[from] async_nats::SubjectError),
    #[error("peer event position lookup failed: {0}")]
    Position(#[from] StreamStoreError),
}

#[derive(Debug, Clone)]
pub struct PeerSubjectResolver {
    tenant: PeerTenant,
    scope: SubjectScope,
}

impl PeerSubjectResolver {
    pub fn new(tenant: PeerTenant) -> Result<Self, PeerSubjectError> {
        let scope = SubjectScope::new(format!("{}.peer.v1.peer", tenant.as_str()))?;
        Ok(Self { tenant, scope })
    }

    pub fn tenant(&self) -> &PeerTenant {
        &self.tenant
    }

    pub fn subject(&self, _peer_id: &PeerId) -> Result<StreamSubject, PeerSubjectError> {
        Err(PeerSubjectError::Unimplemented)
    }
}

impl StreamSubjectResolver<PeerId> for PeerSubjectResolver {
    type Error = PeerSubjectError;

    fn subject_scope(&self) -> Option<&SubjectScope> {
        Some(&self.scope)
    }

    async fn resolve_subject_state(
        &self,
        _events_stream: &jetstream::stream::Stream,
        _stream_id: &PeerId,
    ) -> Result<SubjectState, Self::Error> {
        Err(PeerSubjectError::Unimplemented)
    }
}
