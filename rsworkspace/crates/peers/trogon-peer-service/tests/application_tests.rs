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
        .reconcile(&f.alice, &sender, &[id.clone()], WorkLimit::new(1)?)
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

#[tokio::test]
async fn prepared_assignment_replays_cached_exact_payload_and_frame_over_reconstruction() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("key", "<untrusted> & \"quoted\"").await?;
    let first = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert!(first.batch.is_set());
    assert_eq!(first.messages[0].text.as_deref(), Some("<untrusted> & \"quoted\""));
    let cached = f
        .cache
        .records
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .values()
        .next()
        .cloned()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert!(cached.frame().contains("\\u003c"));
    f.content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clear();
    let replay = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(first, replay);
    Ok(())
}

#[tokio::test]
async fn cache_failure_does_not_commit_preparation_and_frame_corruption_fails_closed() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("key", "payload").await?;
    let before = f.store.appends.load(Ordering::SeqCst);
    f.cache.fail_insert.store(true, Ordering::SeqCst);
    assert!(f.app().prepare_batch(&f.bob, f.prepare()).await.is_err());
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before);
    f.cache.fail_insert.store(false, Ordering::SeqCst);
    let first = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    {
        let mut records = f
            .cache
            .records
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
        for record in records.values_mut() {
            *record = trogon_peer_service::PreparedPresentation::from_port(
                record.batch().clone(),
                record.messages().to_vec(),
                "incompatible frame".into(),
            );
        }
    }
    let unavailable = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(unavailable.batch, first.batch);
    assert!(unavailable.messages.is_empty());
    assert_eq!(
        unavailable.content_availability.and_then(|v| v.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE)
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before + 1);
    Ok(())
}

#[tokio::test]
async fn escaped_rendering_selects_fitting_oldest_prefix_before_preparation() -> TestResult {
    let f = Fixture::new().await?;
    let first = f.intake("first", &"<".repeat(900)).await?;
    f.intake("second", &"<".repeat(900)).await?;
    let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].metadata, first.message);
    Ok(())
}
