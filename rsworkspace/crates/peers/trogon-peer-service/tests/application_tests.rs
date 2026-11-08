use std::sync::atomic::Ordering;
use trogon_peer::{MessageId, PeerId};
use trogon_peer_service::WorkLimit;
mod support;
use support::*;
use trogonai_proto::peers::v1alpha1 as proto;

#[tokio::test]
async fn durable_acceptance_survives_reconstruction_and_exact_retry_with_new_content_location() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f
        .app()
        .send(&f.alice, f.send("key", "hello <peer> & \"world\""))
        .await?;
    let retry = f
        .app()
        .send(&f.alice, f.send("key", "hello <peer> & \"world\""))
        .await?;
    assert_eq!(sent, retry);
    let status = f
        .app()
        .status(
            &f.alice,
            proto::StatusRequest {
                attachment: f.a.clone().into(),
                idempotency_key: Some("key".into()),
            },
        )
        .await?;
    assert_eq!(status.message, sent.message);
    assert_eq!(
        status.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_ACCEPTED)
    );
    assert!(f.app().send(&f.alice, f.send("key", "different")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn denied_acceptance_touches_no_store_content_cache_or_catalog() -> TestResult {
    let f = Fixture::new().await?;
    let before = (
        f.store.reads.load(Ordering::SeqCst),
        f.store.appends.load(Ordering::SeqCst),
    );
    assert!(f.app().send(&f.bob, f.send("denied", "private")).await.is_err());
    assert_eq!(
        before,
        (
            f.store.reads.load(Ordering::SeqCst),
            f.store.appends.load(Ordering::SeqCst)
        )
    );
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn explicit_reconciliation_intakes_from_trusted_history_without_settling_early() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("key", "pending")).await?;
    let id = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.app()
        .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
        .await?;
    f.app().reconcile(&f.alice, &sender, &[id], WorkLimit::new(1)?).await?;
    let status = f
        .app()
        .status(
            &f.alice,
            proto::StatusRequest {
                attachment: f.a.clone().into(),
                idempotency_key: Some("key".into()),
            },
        )
        .await?;
    assert_eq!(
        status.status.and_then(|v| v.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED)
    );
    Ok(())
}

struct ExpireDuringContentOpen {
    inner: DisposableContentFixture,
    clock: ClockFixture,
    expiry: chrono::DateTime<chrono::Utc>,
}

impl trogon_peer_service::ContentOpen for ExpireDuringContentOpen {
    async fn open(
        &self,
        scope: &trogon_peer_service::PeerAccess,
        binding: &trogon_peer_service::ContentBinding,
    ) -> Result<Option<trogon_peer_service::MessageText>, trogon_peer_service::PeerPortError> {
        let text = trogon_peer_service::ContentOpen::open(&self.inner, scope, binding).await?;
        tokio::task::yield_now().await;
        *self
            .clock
            .time
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? = self.expiry;
        Ok(text)
    }
}

async fn replay_reconciliation_receipt(
    store: &StoreFixture,
    peer: &PeerId,
) -> TestResult<trogonai_proto::peers::state_v1alpha1::PeerState> {
    use trogon_decider_runtime::StreamRead;
    let history = store
        .read_stream(trogon_decider_runtime::ReadStreamRequest {
            stream_id: peer,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in history.events {
        let event = event
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        state = trogon_peer::commands::state::evolve(state, &event)?;
    }
    Ok(state)
}

#[tokio::test]
async fn reconciliation_crossing_expiry_during_content_open_records_terminal_receipt() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("expiry-race", "pending")).await?;
    let id = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let recipient = PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let expiry = trogonai_proto::convert::datetime_from_timestamp(
        sent.message
            .expires_at
            .as_option()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let content = ExpireDuringContentOpen {
        inner: f.content.clone(),
        clock: f.clock.clone(),
        expiry,
    };
    let app = trogon_peer_service::PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        content,
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );

    assert_eq!(
        app.reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await?,
        1
    );
    let target = replay_reconciliation_receipt(&f.store, &recipient).await?;
    assert!(target.inbox.is_empty(), "expired content must not be intaken");
    assert_eq!(target.dispositions.len(), 1);
    let receipt = target
        .dispositions
        .first()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert_eq!(receipt.message_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        receipt.outcome.and_then(|value| value.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED)
    );
    let source = replay_reconciliation_receipt(&f.store, &sender).await?;
    let settled = source
        .outbox
        .first()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert_eq!(settled.settlement.as_option(), Some(receipt));
    Ok(())
}
