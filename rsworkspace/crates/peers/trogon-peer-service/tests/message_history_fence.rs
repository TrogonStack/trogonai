use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use trogon_decider_runtime::{
    AppendFailure, AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadStreamRequest, ReadStreamResponse,
    StreamAppend, StreamRead,
};
use trogon_peer_service::{HistoryError, PeerApplication, PeerApplicationError, PeerPortError};
mod support;
use support::*;

#[derive(Debug, thiserror::Error)]
enum InterleavingError {
    #[error("test interleaving could not establish its precondition")]
    Invariant,
    #[error(transparent)]
    Store(<StoreFixture as StreamAppend<trogon_peer::PeerId>>::Error),
}

#[derive(Clone)]
struct UnknownAuthorityBeforeMessageAppend {
    inner: StoreFixture,
    armed: Arc<AtomicBool>,
}

impl trogon_peer_service::ScopedPeerStore for UnknownAuthorityBeforeMessageAppend {
    fn tenant(&self) -> &trogon_peer::TenantId {
        &self.inner.tenant
    }
}

impl StreamRead<trogon_peer::PeerId> for UnknownAuthorityBeforeMessageAppend {
    type Error = <StoreFixture as StreamRead<trogon_peer::PeerId>>::Error;

    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream(request).await
    }

    async fn read_stream_bounded(
        &self,
        request: ReadStreamRequest<'_, trogon_peer::PeerId>,
        maximum: u64,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream_bounded(request, maximum).await
    }
}

impl StreamAppend<trogon_peer::PeerId> for UnknownAuthorityBeforeMessageAppend {
    type Error = InterleavingError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        if self.armed.swap(false, Ordering::SeqCst) {
            let snapshot = self
                .inner
                .read_stream(ReadStreamRequest {
                    stream_id: request.stream_id,
                    from: ReadFrom::Beginning,
                })
                .await
                .map_err(|_| InterleavingError::Invariant)?;
            let mut unknown = snapshot
                .events
                .first()
                .ok_or(InterleavingError::Invariant)?
                .event
                .clone();
            unknown.id = "00000000-0000-4000-8000-fffffffffff3"
                .parse()
                .map_err(|_| InterleavingError::Invariant)?;
            unknown.r#type = "trogonai.peers.future.PeerAuthorityTransferred".into();
            self.inner
                .append_stream(AppendStreamRequest {
                    stream_id: request.stream_id,
                    stream_write_precondition: snapshot.current_position.into(),
                    events: vec![unknown],
                })
                .await
                .map_err(InterleavingError::Store)?;
        }
        self.inner
            .append_stream(request)
            .await
            .map_err(InterleavingError::Store)
    }

    fn classify_append_failure(&self, error: &Self::Error) -> AppendFailure {
        match error {
            InterleavingError::Store(error) => self.inner.classify_append_failure(error),
            InterleavingError::Invariant => AppendFailure::Fatal,
        }
    }
}

#[tokio::test]
async fn a_message_conflict_revalidates_unknown_authority_before_any_retry() -> TestResult {
    let f = Fixture::new().await?;
    assert!(
        f.app()
            .send(&f.bob, f.send("wrong-owner", "reject before storage"))
            .await
            .is_err()
    );
    let armed = Arc::new(AtomicBool::new(true));
    let app = PeerApplication::new(
        UnknownAuthorityBeforeMessageAppend {
            inner: f.store.clone(),
            armed: armed.clone(),
        },
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    assert!(matches!(
        app.send(&f.alice, f.send("unknown-race", "must fail closed")).await,
        Err(PeerApplicationError::History(HistoryError::UnknownEvent))
    ));
    assert!(!armed.load(Ordering::SeqCst));
    let sender = trogon_peer::PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let history = f
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &sender,
            from: ReadFrom::Beginning,
        })
        .await?;
    assert_eq!(
        history.events.len(),
        3,
        "only enrollment and the unknown event may commit"
    );
    assert_eq!(
        history.events[2].event.r#type,
        "trogonai.peers.future.PeerAuthorityTransferred"
    );
    Ok(())
}
