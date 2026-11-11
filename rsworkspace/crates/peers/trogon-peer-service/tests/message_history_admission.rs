use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use trogon_decider_runtime::{AppendStreamRequest, ReadFrom, ReadStreamRequest, StreamAppend, StreamRead};
use trogon_peer_service::{
    ContentBinding, ContentSeal, HistoryError, MessageText, PeerAccess, PeerApplication, PeerApplicationError,
    PeerPortError,
};
mod support;
use support::*;

struct UnknownAuthorityDuringSeal {
    inner: DisposableContentFixture,
    store: StoreFixture,
    sender: trogon_peer::PeerId,
    injected: Arc<AtomicBool>,
}

impl ContentSeal for UnknownAuthorityDuringSeal {
    async fn seal(&self, scope: &PeerAccess, text: &MessageText) -> Result<ContentBinding, PeerPortError> {
        let binding = self.inner.seal(scope, text).await?;
        let snapshot = self
            .store
            .read_stream(ReadStreamRequest {
                stream_id: &self.sender,
                from: ReadFrom::Beginning,
            })
            .await
            .map_err(|_| PeerPortError::Integrity)?;
        let mut unknown = snapshot.events.first().ok_or(PeerPortError::Integrity)?.event.clone();
        unknown.id = "00000000-0000-4000-8000-fffffffffff4"
            .parse()
            .map_err(|_| PeerPortError::Integrity)?;
        unknown.r#type = "trogonai.peers.future.PeerAuthorityTransferred".into();
        self.store
            .append_stream(AppendStreamRequest {
                stream_id: &self.sender,
                stream_write_precondition: snapshot.current_position.into(),
                events: vec![unknown],
            })
            .await
            .map_err(|_| PeerPortError::Integrity)?;
        self.injected.store(true, Ordering::SeqCst);
        Ok(binding)
    }
}

#[tokio::test]
async fn send_rejects_unknown_authority_committed_during_content_sealing() -> TestResult {
    let f = Fixture::new().await?;
    assert!(
        f.app()
            .send(&f.bob, f.send("wrong-owner", "reject before storage"))
            .await
            .is_err()
    );
    let sender = trogon_peer::PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let injected = Arc::new(AtomicBool::new(false));
    let app = PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        UnknownAuthorityDuringSeal {
            inner: f.content.clone(),
            store: f.store.clone(),
            sender: sender.clone(),
            injected: injected.clone(),
        },
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let result = app.send(&f.alice, f.send("unknown-seal", "fail closed")).await;
    assert!(
        injected.load(Ordering::SeqCst),
        "the public content seam must commit the unknown event"
    );
    assert!(
        matches!(result, Err(PeerApplicationError::History(HistoryError::UnknownEvent))),
        "unknown authority must prevent acceptance: {result:?}"
    );
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
        "no message may commit after the unknown authority event"
    );
    Ok(())
}
