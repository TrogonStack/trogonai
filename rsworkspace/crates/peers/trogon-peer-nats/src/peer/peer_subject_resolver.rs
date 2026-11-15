use async_nats::jetstream;
use trogon_decider_nats::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectScope, SubjectScopeError, SubjectState,
    subject_current_position,
};
use trogon_peer::PeerId;

use crate::PeerTenant;

#[derive(Debug, thiserror::Error)]
pub enum PeerSubjectError {
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

    pub fn subject(&self, peer_id: &PeerId) -> Result<StreamSubject, PeerSubjectError> {
        StreamSubject::new(format!("{}.peer.v1.peer.{peer_id}.events", self.tenant.as_str()))
            .map_err(PeerSubjectError::Subject)
    }
}

impl StreamSubjectResolver<PeerId> for PeerSubjectResolver {
    type Error = PeerSubjectError;

    fn subject_scope(&self) -> Option<&SubjectScope> {
        Some(&self.scope)
    }

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &PeerId,
    ) -> Result<SubjectState, Self::Error> {
        let subject = self.subject(stream_id)?;
        let current_position = subject_current_position(events_stream, &subject).await?;
        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}
