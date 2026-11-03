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
async fn optional_prompt_correlation_delivers_and_replays_the_same_assignment() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("optional_prompt", "queued for the next native prompt").await?;
    let mut omitted = f.prepare();
    omitted.prompt_id = None;
    let first = f.app().prepare_batch(&f.bob, omitted).await?;
    assert_eq!(first.messages.len(), 1);
    assert_eq!(
        first.messages[0].text.as_deref(),
        Some("queued for the next native prompt")
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    for correlation in [Some(String::new()), None, Some("another_prompt".into())] {
        let mut request = f.prepare();
        request.prompt_id = correlation;
        assert_eq!(f.app().prepare_batch(&f.bob, request).await?, first);
    }
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}

#[tokio::test]
async fn present_prompt_correlation_uses_native_256_byte_and_control_boundaries() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("prompt_boundary", "bounded correlation").await?;
    let mut maximum = f.prepare();
    maximum.prompt_id = Some("é".repeat(128));
    assert_eq!(f.app().prepare_batch(&f.bob, maximum).await?.messages.len(), 1);
    let appends = f.store.appends.load(Ordering::SeqCst);
    let mut oversized = f.prepare();
    oversized.prompt_id = Some("p".repeat(257));
    assert!(matches!(
        f.app().prepare_batch(&f.bob, oversized).await,
        Err(trogon_peer_service::PeerApplicationError::Value(
            trogon_peer::PeerValueError::TooLong { maximum: 256 }
        ))
    ));
    let mut control = f.prepare();
    control.prompt_id = Some("prompt\nforged".into());
    assert!(matches!(
        f.app().prepare_batch(&f.bob, control).await,
        Err(trogon_peer_service::PeerApplicationError::Value(
            trogon_peer::PeerValueError::InvalidCharacter
        ))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
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

#[tokio::test]
async fn application_ack_is_backend_owned_once_and_distinct_from_sender_settlement() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.intake("key", "act on this").await?;
    let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    assert!(!f.history(&sender).await?.outbox[0].settlement.is_set());
    let id = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.app().reconcile(&f.alice, &sender, &[id], WorkLimit::new(1)?).await?;
    assert!(f.history(&sender).await?.outbox[0].settlement.is_set());
    let ack = proto::AcknowledgeBatchRequest {
        attachment: f.b.clone().into(),
        batch_id: prepared.batch.batch_id.clone(),
    };
    assert!(f.app().acknowledge_batch(&f.alice, ack.clone()).await.is_err());
    let before = f.store.appends.load(Ordering::SeqCst);
    let acknowledged = f.app().acknowledge_batch(&f.bob, ack.clone()).await?;
    assert!(acknowledged.batch.acknowledged_at.is_set());
    assert_eq!(f.app().acknowledge_batch(&f.bob, ack).await?, acknowledged);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before + 1);
    assert!(!f.app().prepare_batch(&f.bob, f.prepare()).await?.batch.is_set());
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
        Some(proto::MessageStatus::MESSAGE_STATUS_APPLICATION_ACKNOWLEDGED)
    );
    Ok(())
}

#[tokio::test]
async fn authorized_roster_paginates_membership_and_observes_liveness_with_service_time() -> TestResult {
    let f = Fixture::new().await?;
    let first = f
        .app()
        .roster(
            &f.alice,
            proto::RosterRequest {
                attachment: f.a.clone().into(),
                page_size: Some(1),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(first.peers.len(), 1);
    let second = f
        .app()
        .roster(
            &f.alice,
            proto::RosterRequest {
                attachment: f.a.clone().into(),
                page_size: Some(1),
                page_token: first.next_page_token,
            },
        )
        .await?;
    assert_eq!(second.peers.len(), 1);
    assert_ne!(first.peers[0].peer_id, second.peers[0].peer_id);
    assert!(!first.peers[0].active_at.is_set());
    let before = f.store.reads.load(Ordering::SeqCst);
    let catalog_calls = f.catalog.calls.load(Ordering::SeqCst);
    f.policy.deny.store(true, Ordering::SeqCst);
    assert!(
        f.app()
            .roster(
                &f.alice,
                proto::RosterRequest {
                    attachment: f.a.clone().into(),
                    page_size: Some(1),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(f.store.reads.load(Ordering::SeqCst), before);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), catalog_calls);
    Ok(())
}

#[tokio::test]
async fn contending_preparers_return_the_same_winner_and_track_the_losing_cache_record() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("key", "one winner").await?;
    *f.store
        .append_barrier
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? =
        Some(std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let left = f.app();
    let right = f.app();
    let (left, right) = tokio::join!(
        left.prepare_batch(&f.bob, f.prepare()),
        right.prepare_batch(&f.bob, f.prepare())
    );
    let left = left?;
    let right = right?;
    assert_eq!(left, right);
    assert_eq!(
        f.cache
            .records
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .len(),
        2
    );
    assert_eq!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn lost_append_reply_keeps_the_possibly_winning_presentation_out_of_orphan_collection() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("key", "durable winner").await?;
    f.store.lose_append_reply.store(true, Ordering::SeqCst);
    assert!(f.app().prepare_batch(&f.bob, f.prepare()).await.is_err());
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .is_empty()
    );
    let replay = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(replay.messages[0].text.as_deref(), Some("durable winner"));
    Ok(())
}

#[tokio::test]
async fn policy_denials_precede_all_ports_for_queries_preparation_ack_and_reconciliation() -> TestResult {
    let f = Fixture::new().await?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let before = (
        f.store.reads.load(Ordering::SeqCst),
        f.store.appends.load(Ordering::SeqCst),
    );
    f.policy.deny.store(true, Ordering::SeqCst);
    assert!(
        f.app()
            .status(
                &f.alice,
                proto::StatusRequest {
                    attachment: f.a.clone().into(),
                    idempotency_key: Some("key".into())
                }
            )
            .await
            .is_err()
    );
    assert!(f.app().prepare_batch(&f.bob, f.prepare()).await.is_err());
    assert!(
        f.app()
            .acknowledge_batch(
                &f.bob,
                proto::AcknowledgeBatchRequest {
                    attachment: f.b.clone().into(),
                    batch_id: Some("batch".into())
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.app()
            .reconcile(&f.alice, &sender, &[MessageId::new("message")?], WorkLimit::new(1)?)
            .await
            .is_err()
    );
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
async fn infrastructure_failure_stays_retryable_but_confirmed_unavailability_has_a_recipient_receipt() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("key", "retry this")).await?;
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
    f.content.fail_open.store(true, Ordering::SeqCst);
    assert!(
        f.app()
            .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await
            .is_err()
    );
    assert!(!f.history(&sender).await?.outbox[0].settlement.is_set());
    assert!(f.history(&recipient).await?.dispositions.is_empty());
    f.content.fail_open.store(false, Ordering::SeqCst);
    f.content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clear();
    f.app().reconcile(&f.alice, &sender, &[id], WorkLimit::new(1)?).await?;
    let state = f.history(&sender).await?;
    assert_eq!(
        state.outbox[0].settlement.outcome.and_then(|v| v.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE)
    );
    Ok(())
}

#[tokio::test]
async fn expiry_disposes_only_unprepared_obligations_and_prepared_winners_survive_expiry() -> TestResult {
    let mut f = Fixture::new().await?;
    let sent = f.intake("key", "keep exact").await?;
    let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    f.advance_and_resume(chrono::Duration::hours(1)).await?;
    f.content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clear();
    assert_eq!(f.app().prepare_batch(&f.bob, f.prepare()).await?, prepared);
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let id = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.app().reconcile(&f.alice, &sender, &[id], WorkLimit::new(1)?).await?;
    assert_eq!(
        f.history(&sender).await?.outbox[0]
            .settlement
            .outcome
            .and_then(|v| v.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED)
    );
    f.cache
        .records
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clear();
    let unavailable = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(unavailable.batch, prepared.batch);
    assert!(unavailable.messages.is_empty());
    assert_eq!(
        unavailable.content_availability.and_then(|v| v.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE)
    );
    let mut unprepared = Fixture::new().await?;
    let sent = unprepared
        .app()
        .send(&unprepared.alice, unprepared.send("key", "expired"))
        .await?;
    let id = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    unprepared.advance_and_resume(chrono::Duration::hours(1)).await?;
    unprepared
        .app()
        .reconcile(&unprepared.alice, &sender, &[id], WorkLimit::new(1)?)
        .await?;
    assert_eq!(
        unprepared.history(&sender).await?.outbox[0]
            .settlement
            .outcome
            .and_then(|v| v.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED)
    );
    Ok(())
}

#[tokio::test]
async fn recipient_overflow_defers_intake_without_deleting_inbox_or_settling_sender() -> TestResult {
    let f = Fixture::new().await?;
    let mut last = None;
    for index in 0..7 {
        let (owner, reference) = f.add_peer(&format!("sender{index}")).await?;
        for number in 0..10 {
            let mut request = f.send(&format!("key{number}"), "bounded");
            request.attachment = reference.clone().into();
            let sent = f.app().send(&owner, request).await?;
            let id = MessageId::new(
                sent.message
                    .message_id
                    .clone()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            )?;
            let sender = PeerId::parse(
                reference
                    .peer_id
                    .clone()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            )?;
            f.app()
                .reconcile(&owner, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
                .await?;
            last = Some((owner.clone(), sender, id));
        }
    }
    let recipient = PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    assert_eq!(f.history(&recipient).await?.inbox.len(), 64);
    let (owner, sender, id) = last.ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    f.app().reconcile(&owner, &sender, &[id], WorkLimit::new(1)?).await?;
    let history = f.history(&sender).await?;
    assert!(!history.outbox[9].settlement.is_set());
    assert_eq!(f.history(&recipient).await?.inbox.len(), 64);
    Ok(())
}

#[tokio::test]
async fn scoped_work_limits_and_cross_tenant_owner_fail_before_port_access() -> TestResult {
    let f = Fixture::new().await?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let before = f.store.reads.load(Ordering::SeqCst);
    assert!(
        f.app()
            .reconcile(
                &f.alice,
                &sender,
                &[MessageId::new("one")?, MessageId::new("two")?],
                WorkLimit::new(1)?
            )
            .await
            .is_err()
    );
    assert_eq!(f.store.reads.load(Ordering::SeqCst), before);
    let intruder = trogon_peer::TrustedPeerOwner::new(
        trogon_peer::TenantId::new("other")?,
        trogon_peer::CallerId::new("alice")?,
    );
    assert!(f.app().send(&intruder, f.send("key", "denied")).await.is_err());
    assert_eq!(f.store.reads.load(Ordering::SeqCst), before);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn immutable_cache_key_reuse_cannot_overwrite_an_acknowledged_assignment() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("first", "original").await?;
    let first = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    f.app()
        .acknowledge_batch(
            &f.bob,
            proto::AcknowledgeBatchRequest {
                attachment: f.b.clone().into(),
                batch_id: first.batch.batch_id.clone(),
            },
        )
        .await?;
    f.intake("second", "replacement").await?;
    let existing = f
        .cache
        .records
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .values()
        .next()
        .cloned()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    f.ids.0.store(1, Ordering::SeqCst);
    let before = f.store.appends.load(Ordering::SeqCst);
    assert!(f.app().prepare_batch(&f.bob, f.prepare()).await.is_err());
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before);
    let unchanged = f
        .cache
        .records
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .values()
        .next()
        .cloned()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert!(existing == unchanged);
    Ok(())
}

#[tokio::test]
async fn separate_collaborations_cannot_accept_content_even_when_peer_reads_are_granted() -> TestResult {
    let f = Fixture::new().await?;
    let owner = trogon_peer::TrustedPeerOwner::new(f.alice.tenant().clone(), trogon_peer::CallerId::new("isolated")?);
    let service = trogon_peer_service::PeerService::new(
        f.store.clone(),
        IsolatedEnrollmentFixture(owner.clone()),
        f.clock.clone(),
    );
    let mut request = Fixture::enrollment("isolated");
    request.collaboration_ref = Some("isolated".into());
    let isolated = service.enroll(&owner, request).await?;
    let id = isolated.peer_id.ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    f.policy
        .grants
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .insert(id.clone(), owner);
    let mut send = f.send("cross", "private");
    send.recipient_peer_id = Some(id);
    let before = f.store.appends.load(Ordering::SeqCst);
    assert!(f.app().send(&f.alice, send).await.is_err());
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn offline_catalog_members_leave_live_roster_and_keep_historical_activity() -> TestResult {
    let mut f = Fixture::new().await?;
    let active = *f
        .clock
        .time
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
    let service = trogon_peer_service::PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
    service
        .heartbeat(
            &f.bob,
            proto::HeartbeatRequest {
                attachment: f.b.clone().into(),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                active_at: trogonai_proto::convert::timestamp_from_datetime(&active).into(),
                ..Default::default()
            },
        )
        .await?;
    *f.clock
        .time
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? += chrono::Duration::seconds(11);
    f.a = service
        .enroll(&f.alice, Fixture::enrollment("alice"))
        .await?
        .attachment
        .attachment
        .clone()
        .into_option()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let roster = f
        .app()
        .roster(
            &f.alice,
            proto::RosterRequest {
                attachment: f.a.clone().into(),
                page_size: Some(64),
                ..Default::default()
            },
        )
        .await?;
    assert!(roster.peers.iter().all(|p| p.peer_id != f.b.peer_id));
    let bob_id = PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let bob = f.history(&bob_id).await?;
    assert_eq!(
        bob.active_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&active))
    );
    let id =
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    f.policy
        .grants
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .remove(&id);
    let restricted = f
        .app()
        .roster(
            &f.alice,
            proto::RosterRequest {
                attachment: f.a.clone().into(),
                page_size: Some(64),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(restricted.peers.len(), 1);
    Ok(())
}

#[tokio::test]
async fn live_roster_filters_expired_and_detached_peers_without_erasing_their_history() -> TestResult {
    let mut f = Fixture::new().await?;
    let service = trogon_peer_service::PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
    let (charlie, c) = f.add_peer("charlie").await?;
    let active = *f
        .clock
        .time
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
    f.intake("expired_inbox", "retain expired connector inbox").await?;
    let mut send = f.send("detached_inbox", "retain detached connector inbox");
    send.recipient_peer_id = c.peer_id.clone();
    let sent = f.app().send(&f.alice, send).await?;
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let message = MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.app()
        .reconcile(&f.alice, &sender, &[message], WorkLimit::new(1)?)
        .await?;
    for (owner, reference) in [(&f.bob, &f.b), (&charlie, &c)] {
        service
            .heartbeat(
                owner,
                proto::HeartbeatRequest {
                    attachment: reference.clone().into(),
                    availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                    active_at: trogonai_proto::convert::timestamp_from_datetime(&active).into(),
                    ..Default::default()
                },
            )
            .await?;
    }
    service
        .detach(
            &charlie,
            proto::DetachRequest {
                attachment: c.clone().into(),
            },
        )
        .await?;
    let expired = PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let detached = PeerId::parse(c.peer_id.clone().ok_or(trogon_peer_service::PeerPortError::Integrity)?)?;
    let expired_history = f.history(&expired).await?;
    let detached_history = f.history(&detached).await?;
    assert_eq!(expired_history.inbox.len(), 1);
    assert_eq!(detached_history.inbox.len(), 1);
    assert_eq!(expired_history.active_at, detached_history.active_at);
    assert!(expired_history.active_at.is_set());
    *f.clock
        .time
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? += chrono::Duration::seconds(10);
    f.a = service
        .enroll(&f.alice, Fixture::enrollment("alice"))
        .await?
        .attachment
        .attachment
        .clone()
        .into_option()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let (_, live) = f.add_peer("dana").await?;
    let mut catalog = f
        .catalog
        .peers
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clone();
    catalog.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let mut cursor = None;
    let mut live_ids = Vec::new();
    for member in &catalog {
        let page = f
            .app()
            .roster(
                &f.alice,
                proto::RosterRequest {
                    attachment: f.a.clone().into(),
                    page_size: Some(1),
                    page_token: cursor,
                },
            )
            .await?;
        assert_eq!(page.next_page_token.as_deref(), Some(member.as_str()));
        if member == &expired || member == &detached {
            assert!(
                page.peers.is_empty(),
                "nonlive catalog members must not appear in the live roster"
            );
        } else {
            assert_eq!(page.peers.len(), 1);
            assert!(page.peers[0].availability.is_some());
            live_ids.push(
                page.peers[0]
                    .peer_id
                    .clone()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            );
        }
        cursor = page.next_page_token;
    }
    let last = f
        .app()
        .roster(
            &f.alice,
            proto::RosterRequest {
                attachment: f.a.clone().into(),
                page_size: Some(1),
                page_token: cursor,
            },
        )
        .await?;
    assert!(last.peers.is_empty());
    assert!(last.next_page_token.is_none());
    let mut expected = vec![
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        live.peer_id.ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    ];
    expected.sort();
    assert_eq!(live_ids, expected);
    assert_eq!(f.history(&expired).await?, expired_history);
    assert_eq!(f.history(&detached).await?, detached_history);
    Ok(())
}

#[tokio::test]
async fn lost_send_append_reply_retries_the_original_durable_obligation() -> TestResult {
    let f = Fixture::new().await?;
    f.store.lose_append_reply.store(true, Ordering::SeqCst);
    assert!(f.app().send(&f.alice, f.send("key", "accepted once")).await.is_err());
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let accepted = f.history(&sender).await?.outbox[0].message.clone();
    let retried = f.app().send(&f.alice, f.send("key", "accepted once")).await?;
    assert_eq!(retried.message, accepted);
    assert_eq!(f.history(&sender).await?.outbox.len(), 1);
    Ok(())
}

#[tokio::test]
async fn lifetime_commands_refuse_a_store_scoped_to_a_different_tenant_before_io() -> TestResult {
    let f = Fixture::new().await?;
    let mut wrong_store = f.store.clone();
    wrong_store.tenant = trogon_peer::TenantId::new("other")?;
    let service = trogon_peer_service::PeerService::new(wrong_store, f.policy.clone(), f.clock.clone());
    let before = (
        f.store.reads.load(Ordering::SeqCst),
        f.store.appends.load(Ordering::SeqCst),
    );
    assert!(service.enroll(&f.alice, Fixture::enrollment("alice")).await.is_err());
    assert!(
        service
            .heartbeat(
                &f.alice,
                proto::HeartbeatRequest {
                    attachment: f.a.clone().into(),
                    availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        service
            .detach(
                &f.alice,
                proto::DetachRequest {
                    attachment: f.a.clone().into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        before,
        (
            f.store.reads.load(Ordering::SeqCst),
            f.store.appends.load(Ordering::SeqCst)
        )
    );
    Ok(())
}
