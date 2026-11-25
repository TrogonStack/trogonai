use super::peer_subject_resolver::PeerSubjectResolver;
use trogon_decider_nats::JetStreamStore;
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, StreamAppend,
    StreamRead,
};
use trogon_peer::{PeerId, PeerValueError, TenantId};
use trogon_peer_service::ScopedPeerStore;

#[derive(Clone)]
pub struct PeerJetStreamStore {
    inner: JetStreamStore<PeerSubjectResolver>,
    tenant: TenantId,
}
impl PeerJetStreamStore {
    pub fn new(inner: JetStreamStore<PeerSubjectResolver>) -> Result<Self, PeerValueError> {
        let tenant = TenantId::new(inner.subject_resolver().tenant().as_str())?;
        Ok(Self { inner, tenant })
    }
}
impl ScopedPeerStore for PeerJetStreamStore {
    fn tenant(&self) -> &TenantId {
        &self.tenant
    }
}
impl StreamRead<PeerId> for PeerJetStreamStore {
    type Error = <JetStreamStore<PeerSubjectResolver> as StreamRead<PeerId>>::Error;
    async fn read_stream(&self, request: ReadStreamRequest<'_, PeerId>) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream(request).await
    }
    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, PeerId>,
        max: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream_bounded(request, max).await
    }
}
impl StreamAppend<PeerId> for PeerJetStreamStore {
    type Error = <JetStreamStore<PeerSubjectResolver> as StreamAppend<PeerId>>::Error;
    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.inner.append_stream(request).await
    }
    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        self.inner.classify_append_failure(error)
    }
}
