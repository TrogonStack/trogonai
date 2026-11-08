use chrono::Duration;
use chrono::TimeZone;
use chrono::Timelike;
use std::sync::atomic::Ordering;
use trogon_decider_runtime::CommandAuthorizer as _;
use trogon_decider_runtime::CommandError;
use trogon_decider_runtime::CommandExecution;
use trogon_decider_runtime::CommandPrincipal;
use trogon_decider_runtime::EventEncode;
use trogon_decider_runtime::PrincipalId;
use trogon_decider_runtime::PrincipalKind;
use trogon_decider_runtime::ReadFrom;
use trogon_decider_runtime::ReadStreamRequest;
use trogon_decider_runtime::ReplayLimit;
use trogon_decider_runtime::StreamAppend;
use trogon_decider_runtime::StreamRead;
use trogon_peer::AcceptedMessage;
use trogon_peer::AcknowledgeBatch;
use trogon_peer::AttachmentRef;
use trogon_peer::BatchId;
use trogon_peer::ConnectorId;
use trogon_peer::EvolveError;
use trogon_peer::IntakeMessage;
use trogon_peer::MessageError;
use trogon_peer::PeerValueError;
use trogon_peer::PrepareBatch;
use trogon_peer::SealedContentRef;
use trogon_peer::TrustedBatchPlan;
use trogon_peer::{MessageId, PeerId};
use trogon_peer_service::ContentBinding;
use trogon_peer_service::ContentOpen;
use trogon_peer_service::ContentSeal as _;
use trogon_peer_service::HistoryError;
use trogon_peer_service::MessageText;
use trogon_peer_service::PeerAccess;
use trogon_peer_service::PeerApplication;
use trogon_peer_service::PeerApplicationError;
use trogon_peer_service::PeerClock;
use trogon_peer_service::PeerOperation;
use trogon_peer_service::PeerService;
use trogon_peer_service::PreparedPresentation;
use trogon_peer_service::PresentationInsert;
use trogon_peer_service::PresentationKey;
use trogon_peer_service::PresentationRead;
use trogon_peer_service::ScopedPeerStore;
use trogon_peer_service::{PeerPortError, WorkLimit};
use trogonai_proto::peers::PeerEventCase;
mod support;
use support::*;
use trogonai_proto::peers::v1alpha1 as proto;

#[tokio::test]
async fn send_acceptance_uses_current_service_time_after_a_heartbeat_during_seal() -> TestResult {
    let f = Fixture::new().await?;
    let seal = advancing_seal::AdvancingSeal {
        fixture: &f,
        elapsed: chrono::Duration::seconds(1),
        heartbeat: Some(proto::HeartbeatRequest {
            attachment_ref: f.a.clone().into(),
            availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
            ..Default::default()
        }),
    };
    let app = trogon_peer_service::PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        seal,
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let sent = app
        .send(
            &f.alice,
            f.send("heartbeat_during_seal", "concurrent activity is valid"),
        )
        .await?;
    let now = *f
        .clock
        .time
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
    assert_eq!(
        sent.message.accepted_at.as_option(),
        Some(&trogonai_proto::convert::timestamp_from_datetime(&now))
    );
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let state = f.history(&sender).await?;
    assert_eq!(
        state.attachment.observed_at.as_option(),
        sent.message.accepted_at.as_option()
    );
    assert_eq!(state.outbox.len(), 1);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 1);
    let appends = f.store.appends.load(Ordering::SeqCst);
    let retry = app
        .send(
            &f.alice,
            f.send("heartbeat_during_seal", "concurrent activity is valid"),
        )
        .await?;
    assert_eq!(retry, sent);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    Ok(())
}

#[tokio::test]
async fn send_acceptance_still_denies_a_lease_that_expires_during_seal() -> TestResult {
    let f = Fixture::new().await?;
    let seal = advancing_seal::AdvancingSeal {
        fixture: &f,
        elapsed: chrono::Duration::seconds(10),
        heartbeat: None,
    };
    let app = trogon_peer_service::PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        seal,
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        app.send(&f.alice, f.send("expired_during_seal", "must remain fenced"))
            .await,
        Err(trogon_peer_service::PeerApplicationError::Evidence(
            trogon_peer::MessageError::Guard(trogon_peer::HeartbeatError::Expired)
        ))
    ));
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    assert!(f.history(&sender).await?.outbox.is_empty());
    Ok(())
}

#[tokio::test]
async fn send_render_admission_rejects_unrenderable_new_content_before_sealing_or_acceptance() -> TestResult {
    let f = Fixture::new().await?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app().send(&f.alice, f.send("unrenderable", &"<".repeat(2048))).await,
        Err(trogon_peer_service::PeerApplicationError::Context(
            trogon_peer_context::PeerContextError::ContractViolation
        ))
    ));
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    let sender = PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    assert!(f.history(&sender).await?.outbox.is_empty());
    Ok(())
}

#[tokio::test]
async fn send_render_admission_reserves_full_batch_identity_and_preserves_exact_retries() -> TestResult {
    let f = Fixture::new().await?;
    let now = *f
        .clock
        .time
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
    let admission_at = now
        .with_nanosecond(999_999_999)
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let mut batch = proto::PeerBatch {
        batch_id: Some(trogon_peer::BatchId::new("b".repeat(128))?.as_str().to_owned()),
        message_ids: vec!["message-0".into()],
        prepared_at: trogonai_proto::convert::timestamp_from_datetime(&now).into(),
        ..Default::default()
    };
    let mut candidate = proto::PreparedMessage {
        metadata: proto::MessageMetadata {
            message_id: Some("message-0".into()),
            sender_peer_id: f.a.peer_id.clone(),
            recipient_peer_id: f.b.peer_id.clone(),
            collaboration_ref: Some("shared".into()),
            accepted_at: trogonai_proto::convert::timestamp_from_datetime(&admission_at).into(),
            ..Default::default()
        }
        .into(),
        text: None,
    };
    let mut fitting = None;
    for suffix in 0..=848 {
        let text = format!("{}{}", "<".repeat(1200), "a".repeat(suffix));
        candidate.text = Some(text.clone());
        candidate
            .metadata
            .as_option_mut()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
            .content_size_bytes = Some(text.len() as u32);
        if trogon_peer_context::PeerContext::render(&batch, std::slice::from_ref(&candidate)).is_err() {
            break;
        }
        fitting = Some(text);
    }
    let fitting = fitting.ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    assert!(fitting.len() < 2048);
    let oversized = format!("{fitting}a");
    candidate.text = Some(oversized.clone());
    candidate
        .metadata
        .as_option_mut()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?
        .content_size_bytes = Some(oversized.len() as u32);
    assert!(trogon_peer_context::PeerContext::render(&batch, std::slice::from_ref(&candidate)).is_err());
    batch.batch_id = Some("batch-1".into());
    assert!(trogon_peer_context::PeerContext::render(&batch, &[candidate]).is_ok());
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .send(&f.alice, f.send("too_large_future_batch", &oversized))
            .await,
        Err(trogon_peer_service::PeerApplicationError::Context(
            trogon_peer_context::PeerContextError::ContractViolation
        ))
    ));
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    let accepted = f.app().send(&f.alice, f.send("tight_fit", &fitting)).await?;
    f.ids.0.store(usize::MAX - 1, Ordering::SeqCst);
    let retry = f.app().send(&f.alice, f.send("tight_fit", &fitting)).await?;
    assert_eq!(retry, accepted);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    Ok(())
}

#[tokio::test]
async fn send_render_admission_keeps_normal_and_fitting_escaped_exchange_metadata_exact() -> TestResult {
    for text in ["a".repeat(2048), "<".repeat(1000)] {
        let f = Fixture::new().await?;
        let accepted = f.intake("fits", &text).await?;
        let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
        assert_eq!(prepared.messages.len(), 1);
        assert_eq!(prepared.messages[0].metadata, accepted.message);
        assert_eq!(prepared.messages[0].text.as_deref(), Some(text.as_str()));
        let mut worst_batch = prepared
            .batch
            .into_option()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        worst_batch.batch_id = Some(trogon_peer::BatchId::new("b".repeat(128))?.as_str().to_owned());
        assert!(trogon_peer_context::PeerContext::render(&worst_batch, &prepared.messages).is_ok());
    }
    Ok(())
}

#[tokio::test]
async fn send_render_admission_keeps_existing_key_semantic_conflicts_owned_by_core() -> TestResult {
    let f = Fixture::new().await?;
    let accepted = f.app().send(&f.alice, f.send("accepted", "original")).await?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    let seals = f.content.calls.load(Ordering::SeqCst);
    assert!(matches!(
        f.app().send(&f.alice, f.send("accepted", &"<".repeat(2048))).await,
        Err(trogon_peer_service::PeerApplicationError::Message(
            trogon_decider_runtime::CommandError::Decide(trogon_peer::MessageError::Conflict)
        ))
    ));
    assert_eq!(f.content.calls.load(Ordering::SeqCst), seals + 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.app().send(&f.alice, f.send("accepted", "original")).await?, accepted);
    Ok(())
}

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
                attachment_ref: f.a.clone().into(),
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
async fn content_ports_preserve_semantics_but_verify_storage_integrity_and_scope_independently() -> TestResult {
    let f = Fixture::new().await?;
    let peer = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let scope = PeerAccess::new(f.alice.clone(), peer, PeerOperation::Send);
    let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(f.alice.caller().as_str())?);
    f.policy.authorize(&principal, &scope)?;
    let text = MessageText::new("fixture stored bytes with é".into())?;
    let first = f.content.seal(&scope, &text).await?;
    let second = f.content.seal(&scope, &text).await?;
    assert_ne!(
        first.content().reference().as_str(),
        second.content().reference().as_str()
    );
    assert_eq!(
        first.content().semantic_fingerprint(),
        second.content().semantic_fingerprint()
    );
    let integrity = <sha2::Sha256 as sha2::Digest>::digest(text.as_str().as_bytes());
    assert_eq!(first.content().reference().integrity_digest(), &integrity[..]);
    assert_ne!(
        first.content().semantic_fingerprint(),
        first.content().reference().integrity_digest()
    );
    assert!(f.content.open(&scope, &first).await? == Some(text.clone()));
    let mut corrupted = first.content().reference().to_claim_check();
    corrupted
        .digest
        .as_option_mut()
        .ok_or(PeerPortError::Integrity)?
        .value
        .as_mut()
        .ok_or(PeerPortError::Integrity)?[0] ^= 1;
    let bad_integrity = ContentBinding::from_port(
        f.alice.tenant().clone(),
        SealedContentRef::new(corrupted)?,
        first.content().size(),
        first.content().semantic_fingerprint().to_vec(),
    )?;
    assert!(
        matches!(
            f.content.open(&scope, &bad_integrity).await,
            Err(PeerPortError::Integrity)
        ),
        "valid semantic token must not authenticate mismatched stored bytes"
    );
    let bad_size = ContentBinding::from_port(
        f.alice.tenant().clone(),
        first.content().reference().clone(),
        first.content().size().checked_add(1).ok_or(PeerPortError::Integrity)?,
        first.content().semantic_fingerprint().to_vec(),
    )?;
    assert!(
        matches!(f.content.open(&scope, &bad_size).await, Err(PeerPortError::Integrity)),
        "matching ClaimCheck and fingerprint must not validate the wrong declared UTF-8 size"
    );
    let mut fingerprint = first.content().semantic_fingerprint().to_vec();
    fingerprint[0] ^= 1;
    let bad_semantics = ContentBinding::from_port(
        f.alice.tenant().clone(),
        first.content().reference().clone(),
        first.content().size(),
        fingerprint,
    )?;
    assert!(
        matches!(
            f.content.open(&scope, &bad_semantics).await,
            Err(PeerPortError::Integrity)
        ),
        "valid storage digest must not replace the semantic binding"
    );
    let foreign = ContentBinding::from_port(
        trogon_peer::TenantId::new("foreign")?,
        first.content().reference().clone(),
        first.content().size(),
        first.content().semantic_fingerprint().to_vec(),
    )?;
    assert!(matches!(
        f.content.open(&scope, &foreign).await,
        Err(PeerPortError::Denied)
    ));
    Ok(())
}

struct ServiceCoverageIsolatedEnrollment(trogon_peer::TrustedPeerOwner);

impl trogon_decider_runtime::CommandAuthorizer<trogon_peer::Enroll> for ServiceCoverageIsolatedEnrollment {
    fn authorize(
        &self,
        principal: &trogon_decider_runtime::CommandPrincipal,
        command: &trogon_peer::Enroll,
    ) -> Result<(), trogon_decider_runtime::AuthorizationDeniedError> {
        if command.identity().owner() != &self.0
            || command.collaboration().as_str() != "isolated"
            || principal.kind() != trogon_decider_runtime::PrincipalKind::Agent
            || principal.id().as_str() != self.0.caller().as_str()
        {
            return Err(trogon_decider_runtime::AuthorizationDeniedError::new(
                "outside isolated enrollment grant",
            ));
        }
        Ok(())
    }
}

impl trogon_decider_runtime::CommandAuthorizer<trogon_peer::Attach> for ServiceCoverageIsolatedEnrollment {
    fn authorize(
        &self,
        principal: &trogon_decider_runtime::CommandPrincipal,
        command: &trogon_peer::Attach,
    ) -> Result<(), trogon_decider_runtime::AuthorizationDeniedError> {
        if command.identity().owner() != &self.0
            || command.collaboration().as_str() != "isolated"
            || principal.kind() != trogon_decider_runtime::PrincipalKind::Agent
            || principal.id().as_str() != self.0.caller().as_str()
        {
            return Err(trogon_decider_runtime::AuthorizationDeniedError::new(
                "outside isolated enrollment grant",
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn message_byte_limits_reject_invalid_requests_before_ports_and_preserve_utf8() -> TestResult {
    let f = Fixture::new().await?;
    let reads = f.store.reads.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app().send(&f.alice, f.send("empty", "")).await,
        Err(PeerApplicationError::Value(trogon_peer::PeerValueError::InvalidContent))
    ));
    assert!(matches!(
        f.app().send(&f.alice, f.send("oversized", &"é".repeat(1025))).await,
        Err(PeerApplicationError::Value(trogon_peer::PeerValueError::InvalidContent))
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    let payload = "é".repeat(1024);
    assert_eq!(MessageText::new(payload.clone())?.as_str(), payload);
    Ok(())
}

#[test]
fn bounded_work_rejects_empty_and_oversized_batches_without_changing_its_ceiling() -> TestResult {
    assert!(matches!(
        trogon_peer_service::WorkLimit::new(0),
        Err(trogon_peer_service::PeerPortError::Denied)
    ));
    assert!(matches!(
        trogon_peer_service::WorkLimit::new(65),
        Err(trogon_peer_service::PeerPortError::Denied)
    ));
    assert_eq!(trogon_peer_service::WorkLimit::new(64)?.get(), 64);
    Ok(())
}

#[tokio::test]
async fn an_authorized_owner_cannot_use_a_store_bound_to_another_tenant() -> TestResult {
    let f = Fixture::new().await?;
    let mut store = f.store.clone();
    store.tenant = trogon_peer::TenantId::new("different-store-tenant")?;
    let reads = f.store.reads.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    let app = trogon_peer_service::PeerApplication::new(
        store,
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    assert!(matches!(
        app.send(&f.alice, f.send("cross-store", "private")).await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

struct MisroutedPeerReadStore {
    inner: StoreFixture,
    returned_peer: trogon_peer::PeerId,
}

impl trogon_peer_service::ScopedPeerStore for MisroutedPeerReadStore {
    fn tenant(&self) -> &trogon_peer::TenantId {
        &self.inner.tenant
    }
}

impl trogon_decider_runtime::StreamRead<trogon_peer::PeerId> for MisroutedPeerReadStore {
    type Error = <StoreFixture as trogon_decider_runtime::StreamRead<trogon_peer::PeerId>>::Error;
    async fn read_stream(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamRead::read_stream(
            &self.inner,
            trogon_decider_runtime::ReadStreamRequest {
                stream_id: &self.returned_peer,
                from: request.from,
            },
        )
        .await
    }
    async fn read_stream_bounded(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, trogon_peer::PeerId>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamRead::read_stream_bounded(
            &self.inner,
            trogon_decider_runtime::ReadStreamRequest {
                stream_id: &self.returned_peer,
                from: request.from,
            },
            maximum,
        )
        .await
    }
}

impl trogon_decider_runtime::StreamAppend<trogon_peer::PeerId> for MisroutedPeerReadStore {
    type Error = <StoreFixture as trogon_decider_runtime::StreamAppend<trogon_peer::PeerId>>::Error;
    async fn append_stream(
        &self,
        request: trogon_decider_runtime::AppendStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamAppend::append_stream(&self.inner, request).await
    }
    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        trogon_decider_runtime::StreamAppend::classify_append_failure(&self.inner, error)
    }
}

#[tokio::test]
async fn valid_durable_history_returned_for_another_peer_is_refused_before_sealing() -> TestResult {
    let f = Fixture::new().await?;
    let store = MisroutedPeerReadStore {
        inner: f.store.clone(),
        returned_peer: trogon_peer::PeerId::parse(
            f.b.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?,
    };
    let appends = f.store.appends.load(Ordering::SeqCst);
    let app = trogon_peer_service::PeerApplication::new(
        store,
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    assert!(matches!(
        app.send(&f.alice, f.send("misrouted", "private")).await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn denied_detach_preserves_the_attachment_for_a_later_authorized_heartbeat() -> TestResult {
    let f = Fixture::new().await?;
    let service = trogon_peer_service::PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
    let appends = f.store.appends.load(Ordering::SeqCst);
    let reads = f.store.reads.load(Ordering::SeqCst);
    f.policy.deny.store(true, Ordering::SeqCst);
    assert!(matches!(
        service
            .detach(
                &f.bob,
                proto::DetachRequest {
                    attachment_ref: f.b.clone().into()
                }
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Detach(
            trogon_decider_runtime::CommandError::Unauthorized(_)
        ))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads);
    f.policy.deny.store(false, Ordering::SeqCst);
    let heartbeat = service
        .heartbeat(
            &f.bob,
            proto::HeartbeatRequest {
                attachment_ref: f.b.clone().into(),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(
        heartbeat.attachment.status.and_then(|status| status.as_known()),
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
    );
    Ok(())
}

struct ReturnedSealedBinding(trogon_peer_service::ContentBinding);

impl trogon_peer_service::ContentSeal for ReturnedSealedBinding {
    async fn seal(
        &self,
        _scope: &trogon_peer_service::PeerAccess,
        _text: &trogon_peer_service::MessageText,
    ) -> Result<trogon_peer_service::ContentBinding, trogon_peer_service::PeerPortError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn sealing_cannot_change_the_authorized_tenant_or_declared_payload_size() -> TestResult {
    let f = Fixture::new().await?;
    for (tenant, size) in [(trogon_peer::TenantId::new("other")?, 7), (f.alice.tenant().clone(), 6)] {
        let binding = trogon_peer_service::ContentBinding::from_port(
            tenant,
            trogon_peer::SealedContentRef::new(proto::ClaimCheck {
                storage_ref: proto::StorageLocator {
                    value: Some("disposable_invalid_binding".into()),
                }
                .into(),
                digest: proto::Digest {
                    algorithm: Some("sha256".into()),
                    value: Some(<sha2::Sha256 as sha2::Digest>::digest(b"payload").to_vec()),
                }
                .into(),
            })?,
            size,
            vec![1; 32],
        )?;
        let app = trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            ReturnedSealedBinding(binding),
            f.cache.clone(),
            f.ids.clone(),
            f.catalog.clone(),
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        assert!(matches!(
            app.send(&f.alice, f.send("bad-binding", "payload")).await,
            Err(trogon_peer_service::PeerApplicationError::Scope)
        ));
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

async fn service_coverage_public_history(
    f: &Fixture,
    peer: &trogon_peer::PeerId,
) -> TestResult<trogonai_proto::peers::state_v1alpha1::PeerState> {
    let read = f
        .store
        .read_stream(trogon_decider_runtime::ReadStreamRequest {
            stream_id: peer,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in read.events {
        let decoded = event
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        state = trogon_peer::commands::state::evolve(state, &decoded)?;
    }
    Ok(state)
}

#[tokio::test]
async fn status_reports_terminal_backend_dispositions_without_claiming_sender_settlement() -> TestResult {
    for (disposition, expected_status) in [
        (
            trogon_peer::TerminalDisposition::Unavailable,
            proto::MessageStatus::MESSAGE_STATUS_UNAVAILABLE,
        ),
        (
            trogon_peer::TerminalDisposition::Expired,
            proto::MessageStatus::MESSAGE_STATUS_EXPIRED,
        ),
    ] {
        let mut f = Fixture::new().await?;
        let sent = f.app().send(&f.alice, f.send("terminal-status", "pending")).await?;
        let sender = trogon_peer::PeerId::parse(
            f.a.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let id = trogon_peer::MessageId::new(
            sent.message
                .message_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let history = service_coverage_public_history(&f, &sender).await?;
        let evidence = trogon_peer::AcceptedMessage::from_sender_history(&history, &id)?;
        if disposition == trogon_peer::TerminalDisposition::Expired {
            *f.clock
                .time
                .lock()
                .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? =
                trogonai_proto::convert::datetime_from_timestamp(
                    sent.message
                        .expires_at
                        .as_option()
                        .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
                )?;
            let service = trogon_peer_service::PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
            f.a = service
                .enroll(&f.alice, Fixture::enrollment("alice"))
                .await?
                .attachment
                .attachment_ref
                .clone()
                .into_option()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        }
        let now = *f
            .clock
            .time
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
        let command = trogon_peer::DisposeMessage::new(evidence, disposition, now);
        let principal = trogon_decider_runtime::CommandPrincipal::new(
            trogon_decider_runtime::PrincipalKind::Agent,
            trogon_decider_runtime::PrincipalId::new(f.alice.caller().as_str())?,
        );
        trogon_decider_runtime::CommandExecution::new(&f.store, &command)
            .with_principal(principal)
            .with_authorizer(&f.policy)
            .with_replay_limit(trogon_decider_runtime::ReplayLimit::try_new(4096)?)
            .execute()
            .await?;
        let appends = f.store.appends.load(Ordering::SeqCst);
        let status = f
            .app()
            .status(
                &f.alice,
                proto::StatusRequest {
                    attachment_ref: f.a.clone().into(),
                    idempotency_key: Some("terminal-status".into()),
                },
            )
            .await?;
        assert_eq!(status.status.and_then(|v| v.as_known()), Some(expected_status));
        assert_eq!(status.message, sent.message);
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        let source = service_coverage_public_history(&f, &sender).await?;
        assert!(
            !source
                .outbox
                .first()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                .settlement
                .is_set()
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_history_beyond_the_documented_replay_ceiling_fails_closed_at_the_public_query() -> TestResult {
    let f = Fixture::new().await?;
    let service = trogon_peer_service::PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
    service
        .heartbeat(
            &f.alice,
            proto::HeartbeatRequest {
                attachment_ref: f.a.clone().into(),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
                ..Default::default()
            },
        )
        .await?;
    let sender = trogon_peer::PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let read = f
        .store
        .read_stream(trogon_decider_runtime::ReadStreamRequest {
            stream_id: &sender,
            from: trogon_decider_runtime::ReadFrom::Beginning,
        })
        .await?;
    let padding = 4097usize
        .checked_sub(read.events.len())
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let heartbeat = read
        .events
        .last()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?
        .event
        .clone();
    f.store
        .append_stream(trogon_decider_runtime::AppendStreamRequest {
            stream_id: &sender,
            stream_write_precondition: read.current_position.into(),
            events: vec![heartbeat; padding],
        })
        .await?;
    let validated = service_coverage_public_history(&f, &sender).await?;
    assert_eq!(validated.enrollment.peer_id.as_deref(), Some(sender.as_str()));
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .status(
                &f.alice,
                proto::StatusRequest {
                    attachment_ref: f.a.clone().into(),
                    idempotency_key: Some("missing".into()),
                }
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::History(
            trogon_peer_service::HistoryError::Limit
        ))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

async fn independent_cross_collaboration_fixture() -> TestResult<(Fixture, trogon_peer::PeerId, trogon_peer::MessageId)>
{
    let f = Fixture::new().await?;
    let isolated_owner =
        trogon_peer::TrustedPeerOwner::new(f.alice.tenant().clone(), trogon_peer::CallerId::new("isolated")?);
    let service = trogon_peer_service::PeerService::new(
        f.store.clone(),
        ServiceCoverageIsolatedEnrollment(isolated_owner.clone()),
        f.clock.clone(),
    );
    let mut enrollment = Fixture::enrollment("isolated");
    enrollment.collaboration_ref = Some("isolated".into());
    let isolated = service.enroll(&isolated_owner, enrollment).await?;
    let recipient = trogon_peer::PeerId::parse(isolated.peer_id.ok_or(trogon_peer_service::PeerPortError::Integrity)?)?;
    f.policy
        .grants
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .insert(recipient.as_str().to_owned(), isolated_owner);
    let sender = trogon_peer::PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let reference = trogon_peer::AttachmentRef::new(
        sender.clone(),
        trogon_peer::ConnectorId::new(
            f.a.connector_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?,
        f.a.generation.ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let scope = trogon_peer_service::PeerAccess::new(
        f.alice.clone(),
        sender.clone(),
        trogon_peer_service::PeerOperation::Send,
    );
    let content = trogon_peer_service::ContentSeal::seal(
        &f.content,
        &scope,
        &trogon_peer_service::MessageText::new("scoped payload".into())?,
    )
    .await?;
    let message_id = trogon_peer::MessageId::new("cross-collaboration-obligation")?;
    let now = *f
        .clock
        .time
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
    let command = trogon_peer::SendMessage::new(
        f.alice.clone(),
        reference,
        trogon_peer::IdempotencyKey::new("cross-status")?,
        message_id.clone(),
        recipient,
        trogon_peer::CollaborationRef::new("shared")?,
        content.content().clone(),
        None,
        now,
    );
    let principal = trogon_decider_runtime::CommandPrincipal::new(
        trogon_decider_runtime::PrincipalKind::Agent,
        trogon_decider_runtime::PrincipalId::new(f.alice.caller().as_str())?,
    );
    trogon_decider_runtime::CommandExecution::new(&f.store, &command)
        .with_principal(principal)
        .with_authorizer(&f.policy)
        .with_replay_limit(trogon_decider_runtime::ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    Ok((f, sender, message_id))
}

#[tokio::test]
async fn independently_accepted_cross_collaboration_obligations_cannot_leak_status() -> TestResult {
    let (f, _, _) = independent_cross_collaboration_fixture().await?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .status(
                &f.alice,
                proto::StatusRequest {
                    attachment_ref: f.a.clone().into(),
                    idempotency_key: Some("cross-status".into()),
                }
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
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
                attachment_ref: f.a.clone().into(),
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
async fn present_prompt_correlation_uses_existing_128_byte_and_control_boundaries() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("prompt_boundary", "bounded correlation").await?;
    let mut maximum = f.prepare();
    maximum.prompt_id = Some("é".repeat(64));
    assert_eq!(f.app().prepare_batch(&f.bob, maximum).await?.messages.len(), 1);
    let appends = f.store.appends.load(Ordering::SeqCst);
    let mut oversized = f.prepare();
    oversized.prompt_id = Some("p".repeat(129));
    assert!(matches!(
        f.app().prepare_batch(&f.bob, oversized).await,
        Err(trogon_peer_service::PeerApplicationError::Value(
            trogon_peer::PeerValueError::TooLong { maximum: 128 }
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

struct RevokeCommandGrantDuringOpen<'a>(&'a Fixture);

impl trogon_peer_service::ContentOpen for RevokeCommandGrantDuringOpen<'_> {
    async fn open(
        &self,
        scope: &trogon_peer_service::PeerAccess,
        binding: &trogon_peer_service::ContentBinding,
    ) -> Result<Option<trogon_peer_service::MessageText>, trogon_peer_service::PeerPortError> {
        let opened = trogon_peer_service::ContentOpen::open(&self.0.content, scope, binding).await?;
        self.0.policy.deny.store(true, Ordering::SeqCst);
        Ok(opened)
    }
}

#[tokio::test]
async fn policy_revocation_while_opening_content_prevents_both_intake_and_terminal_disposition() -> TestResult {
    for content_available in [true, false] {
        let f = Fixture::new().await?;
        let sent = f.app().send(&f.alice, f.send("revoked-open", "pending")).await?;
        let id = trogon_peer::MessageId::new(
            sent.message
                .message_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let sender = trogon_peer::PeerId::parse(
            f.a.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let recipient = trogon_peer::PeerId::parse(
            f.b.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        if !content_available {
            f.content
                .bodies
                .lock()
                .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
                .clear();
        }
        let app = trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            RevokeCommandGrantDuringOpen(&f),
            f.cache.clone(),
            f.ids.clone(),
            f.catalog.clone(),
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        assert!(matches!(
            app.reconcile(
                &f.alice,
                &sender,
                std::slice::from_ref(&id),
                trogon_peer_service::WorkLimit::new(1)?
            )
            .await,
            Err(trogon_peer_service::PeerApplicationError::Message(
                trogon_decider_runtime::CommandError::Unauthorized(_)
            ))
        ));
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        let target = service_coverage_public_history(&f, &recipient).await?;
        assert!(target.inbox.is_empty());
        assert!(target.dispositions.is_empty());
        let source = service_coverage_public_history(&f, &sender).await?;
        assert!(
            !source
                .outbox
                .first()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                .settlement
                .is_set()
        );
        f.policy.deny.store(false, Ordering::SeqCst);
        assert_eq!(
            f.app()
                .reconcile(&f.alice, &sender, &[id], trogon_peer_service::WorkLimit::new(1)?)
                .await?,
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_incorrect_policy_grant_cannot_transfer_the_durable_sender_identity() -> TestResult {
    let f = Fixture::new().await?;
    let sender = trogon_peer::PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.policy
        .grants
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .insert(sender.as_str().to_owned(), f.alice.clone());
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .reconcile(
                &f.alice,
                &sender,
                &[trogon_peer::MessageId::new("not-an-obligation")?],
                trogon_peer_service::WorkLimit::new(1)?
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn independently_accepted_cross_collaboration_obligations_cannot_reconcile() -> TestResult {
    let (f, sender, message_id) = independent_cross_collaboration_fixture().await?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .reconcile(
                &f.alice,
                &sender,
                &[message_id],
                trogon_peer_service::WorkLimit::new(1)?
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

struct ServiceCoverageRejectSettlement(PolicyFixture);

impl trogon_decider_runtime::CommandAuthorizer<trogon_peer_service::PeerAccess> for ServiceCoverageRejectSettlement {
    fn authorize(
        &self,
        principal: &trogon_decider_runtime::CommandPrincipal,
        command: &trogon_peer_service::PeerAccess,
    ) -> Result<(), trogon_decider_runtime::AuthorizationDeniedError> {
        trogon_decider_runtime::CommandAuthorizer::authorize(&self.0, principal, command)
    }
}

impl trogon_decider_runtime::CommandAuthorizer<trogon_peer::IntakeMessage> for ServiceCoverageRejectSettlement {
    fn authorize(
        &self,
        principal: &trogon_decider_runtime::CommandPrincipal,
        command: &trogon_peer::IntakeMessage,
    ) -> Result<(), trogon_decider_runtime::AuthorizationDeniedError> {
        trogon_decider_runtime::CommandAuthorizer::authorize(&self.0, principal, command)
    }
}

impl trogon_decider_runtime::CommandAuthorizer<trogon_peer::DisposeMessage> for ServiceCoverageRejectSettlement {
    fn authorize(
        &self,
        principal: &trogon_decider_runtime::CommandPrincipal,
        command: &trogon_peer::DisposeMessage,
    ) -> Result<(), trogon_decider_runtime::AuthorizationDeniedError> {
        trogon_decider_runtime::CommandAuthorizer::authorize(&self.0, principal, command)
    }
}

impl trogon_decider_runtime::CommandAuthorizer<trogon_peer::SettleMessage> for ServiceCoverageRejectSettlement {
    fn authorize(
        &self,
        principal: &trogon_decider_runtime::CommandPrincipal,
        command: &trogon_peer::SettleMessage,
    ) -> Result<(), trogon_decider_runtime::AuthorizationDeniedError> {
        trogon_decider_runtime::CommandAuthorizer::authorize(&self.0, principal, command)?;
        Err(trogon_decider_runtime::AuthorizationDeniedError::new(
            "settlement grant withdrawn",
        ))
    }
}

#[tokio::test]
async fn rejected_settlement_preserves_the_authoritative_receipt_for_a_later_authorized_attempt() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("settlement-denied", "pending")).await?;
    let id = trogon_peer::MessageId::new(
        sent.message
            .message_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let sender = trogon_peer::PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let recipient = trogon_peer::PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clear();
    let app = trogon_peer_service::PeerApplication::new(
        f.store.clone(),
        ServiceCoverageRejectSettlement(f.policy.clone()),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        app.reconcile(
            &f.alice,
            &sender,
            std::slice::from_ref(&id),
            trogon_peer_service::WorkLimit::new(1)?
        )
        .await,
        Err(trogon_peer_service::PeerApplicationError::Message(
            trogon_decider_runtime::CommandError::Unauthorized(trogon_decider_runtime::UnauthorizedError::Denied(_))
        ))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    let disposed = service_coverage_public_history(&f, &recipient).await?;
    assert_eq!(disposed.dispositions.len(), 1);
    assert_eq!(disposed.dispositions[0].message_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        disposed.dispositions[0].outcome.and_then(|outcome| outcome.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE)
    );
    let pending = service_coverage_public_history(&f, &sender).await?;
    assert_eq!(pending.outbox.len(), 1);
    assert!(!pending.outbox[0].settlement.is_set());
    assert_eq!(
        f.app()
            .reconcile(
                &f.alice,
                &sender,
                std::slice::from_ref(&id),
                trogon_peer_service::WorkLimit::new(1)?
            )
            .await?,
        1
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 2);
    let settled = service_coverage_public_history(&f, &sender).await?;
    assert!(settled.outbox[0].settlement.is_set());
    let recovered = service_coverage_public_history(&f, &recipient).await?;
    assert_eq!(recovered.dispositions, disposed.dispositions);
    Ok(())
}

#[tokio::test]
async fn operations_without_required_fields_return_typed_errors_before_ports() -> TestResult {
    let f = Fixture::new().await?;
    let app = f.app();
    let before = (
        f.store.reads.load(Ordering::SeqCst),
        f.store.appends.load(Ordering::SeqCst),
    );
    let sender = trogon_peer::PeerId::parse(
        f.a.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    assert!(matches!(
        app.reconcile(&f.alice, &sender, &[], trogon_peer_service::WorkLimit::new(1)?)
            .await,
        Err(PeerApplicationError::Input)
    ));
    assert!(matches!(
        app.prepare_batch(&f.bob, Default::default()).await,
        Err(PeerApplicationError::Input)
    ));
    assert!(matches!(
        app.acknowledge_batch(&f.bob, Default::default()).await,
        Err(PeerApplicationError::Input)
    ));
    assert!(matches!(
        app.roster(&f.bob, Default::default()).await,
        Err(PeerApplicationError::Input)
    ));
    assert_eq!(
        (
            f.store.reads.load(Ordering::SeqCst),
            f.store.appends.load(Ordering::SeqCst)
        ),
        before
    );
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn public_preparation_receipts_drive_status_and_preserve_the_exact_presentation_record() -> TestResult {
    let f = Fixture::new().await?;
    let body = "presentation <body>";
    let sent = f.app().send(&f.alice, f.send("presentation_status", body)).await?;
    let metadata = sent.message.as_option().ok_or(PeerPortError::Integrity)?.clone();
    let id = MessageId::new(metadata.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let sender = PeerId::parse(metadata.sender_peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let recipient = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let reference = AttachmentRef::new(
        recipient.clone(),
        ConnectorId::new(f.b.connector_id.clone().ok_or(PeerPortError::Integrity)?)?,
        f.b.generation.ok_or(PeerPortError::Integrity)?,
    )?;
    let evidence = AcceptedMessage::from_sender_history(&service_coverage_public_history(&f, &sender).await?, &id)?;
    let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(f.bob.caller().as_str())?);
    CommandExecution::new(&f.store, &IntakeMessage::new(evidence, f.clock.now()))
        .with_principal(principal.clone())
        .with_authorizer(&f.policy)
        .execute()
        .await?;
    let intake_status = f
        .app()
        .status(
            &f.alice,
            proto::StatusRequest {
                attachment_ref: f.a.clone().into(),
                idempotency_key: Some("presentation_status".into()),
            },
        )
        .await?;
    assert_eq!(
        intake_status.status.and_then(|status| status.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_INTAKE_RECORDED)
    );
    let batch_id = BatchId::new("presentation_batch")?;
    let draft_batch = proto::PeerBatch {
        batch_id: Some(batch_id.as_str().to_owned()),
        message_ids: vec![id.as_str().to_owned()],
        prepared_at: trogonai_proto::convert::timestamp_from_datetime(&f.clock.now()).into(),
        ..Default::default()
    };
    let messages = vec![proto::PreparedMessage {
        metadata: metadata.into(),
        text: Some(body.into()),
    }];
    let frame = trogon_peer_context::PeerContext::render(&draft_batch, &messages)?;
    let plan = TrustedBatchPlan::from_renderer(vec![id], frame.as_str().len())?;
    let prepared = CommandExecution::new(
        &f.store,
        &PrepareBatch::new(f.bob.clone(), reference.clone(), batch_id.clone(), plan, f.clock.now()),
    )
    .with_principal(principal.clone())
    .with_authorizer(&f.policy)
    .execute()
    .await?
    .state;
    let batch = prepared.batches.first().ok_or(PeerPortError::Integrity)?.clone();
    let record = PreparedPresentation::from_port(batch.clone(), messages.clone(), frame.as_str().to_owned());
    let key = PresentationKey::new(f.bob.tenant().clone(), recipient.clone(), batch_id.clone());
    f.cache.insert(&key, &record).await?;
    let restored = f.cache.read(&key).await?.ok_or(PeerPortError::Integrity)?;
    assert_eq!(restored.batch(), &batch);
    assert_eq!(restored.messages(), messages.as_slice());
    assert_eq!(restored.frame(), frame.as_str());
    assert_eq!(key.tenant(), f.bob.tenant());
    assert_eq!(key.peer(), &recipient);
    assert_eq!(key.batch(), &batch_id);
    let foreign_key = PresentationKey::new(f.bob.tenant().clone(), sender.clone(), batch_id.clone());
    assert!(f.cache.read(&foreign_key).await?.is_none());
    let status_request = proto::StatusRequest {
        attachment_ref: f.a.clone().into(),
        idempotency_key: Some("presentation_status".into()),
    };
    let status = f.app().status(&f.alice, status_request.clone()).await?;
    assert_eq!(
        status.status.and_then(|status| status.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_BATCH_PREPARED)
    );
    CommandExecution::new(
        &f.store,
        &AcknowledgeBatch::new(f.bob.clone(), reference, batch_id, f.clock.now()),
    )
    .with_principal(principal)
    .with_authorizer(&f.policy)
    .execute()
    .await?;
    let status = f.app().status(&f.alice, status_request).await?;
    assert_eq!(
        status.status.and_then(|status| status.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_APPLICATION_ACKNOWLEDGED)
    );
    assert!(
        !service_coverage_public_history(&f, &sender).await?.outbox[0]
            .settlement
            .is_set()
    );
    Ok(())
}

#[tokio::test]
async fn a_granted_recipient_in_another_collaboration_is_rejected_before_sealing() -> TestResult {
    let (f, sender, _) = independent_cross_collaboration_fixture().await?;
    let history = service_coverage_public_history(&f, &sender).await?;
    let recipient = trogon_peer::PeerId::parse(
        history.outbox[0]
            .message
            .recipient_peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    let mut request = f.send("cross_collaboration_send", "private body");
    request.recipient_peer_id = Some(recipient.as_str().to_owned());
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app().send(&f.alice, request).await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}

#[tokio::test]
async fn preparation_accepts_absent_and_empty_optional_prompt_correlation() -> TestResult {
    for prompt in [None, Some(String::new())] {
        let f = Fixture::new().await?;
        f.intake("optional-prompt", "retained context").await?;
        let mut request = f.prepare();
        request.prompt_id = prompt;
        let prepared = f.app().prepare_batch(&f.bob, request.clone()).await?;
        assert_eq!(prepared.messages.len(), 1);
        assert_eq!(prepared.messages[0].text.as_deref(), Some("retained context"));
        assert_eq!(f.app().prepare_batch(&f.bob, request).await?, prepared);
    }
    Ok(())
}

#[tokio::test]
async fn malformed_prompt_correlation_uses_identity_errors_before_port_access() -> TestResult {
    let f = Fixture::new().await?;
    let mut request = f.prepare();
    request.prompt_id = Some("prompt\ninvalid".into());
    let reads = f.store.reads.load(Ordering::SeqCst);
    assert!(matches!(
        f.app().prepare_batch(&f.bob, request).await,
        Err(trogon_peer_service::PeerApplicationError::Value(
            trogon_peer::PeerValueError::InvalidCharacter
        ))
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        trogon_peer_service::PromptId::new(String::new()),
        Err(trogon_peer::PeerValueError::Empty)
    ));
    assert!(matches!(
        trogon_peer_service::PromptId::new("x".repeat(129)),
        Err(trogon_peer::PeerValueError::TooLong { maximum: 128 })
    ));
    Ok(())
}

#[derive(Clone, Copy)]
enum WinningPresentationKey {
    Candidate,
    Distinct,
}

struct ConcurrentPreparationCache {
    inner: PresentationFixture,
    store: StoreFixture,
    policy: PolicyFixture,
    owner: trogon_peer::TrustedPeerOwner,
    attachment: trogon_peer::AttachmentRef,
    mode: WinningPresentationKey,
}

impl trogon_peer_service::PresentationRead for ConcurrentPreparationCache {
    async fn read(
        &self,
        key: &trogon_peer_service::PresentationKey,
    ) -> Result<Option<trogon_peer_service::PreparedPresentation>, trogon_peer_service::PeerPortError> {
        trogon_peer_service::PresentationRead::read(&self.inner, key).await
    }
}

impl trogon_peer_service::PresentationOrphan for ConcurrentPreparationCache {
    async fn orphan(
        &self,
        key: &trogon_peer_service::PresentationKey,
    ) -> Result<(), trogon_peer_service::PeerPortError> {
        trogon_peer_service::PresentationOrphan::orphan(&self.inner, key).await?;
        self.inner
            .records
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .remove(&PresentationFixture::key(key));
        Ok(())
    }
}

impl trogon_peer_service::PresentationInsert for ConcurrentPreparationCache {
    async fn insert(
        &self,
        key: &trogon_peer_service::PresentationKey,
        record: &trogon_peer_service::PreparedPresentation,
    ) -> Result<(), trogon_peer_service::PeerPortError> {
        self.inner.insert(key, record).await?;
        let batch_id = match self.mode {
            WinningPresentationKey::Candidate => key.batch().clone(),
            WinningPresentationKey::Distinct => {
                trogon_peer::BatchId::new("competing-winner").map_err(|_| PeerPortError::Integrity)?
            }
        };
        let mut batch = record.batch().clone();
        batch.batch_id = Some(batch_id.as_str().to_owned());
        let frame = trogon_peer_context::PeerContext::render(&batch, record.messages())
            .map_err(|_| PeerPortError::Integrity)?;
        let winning_key =
            trogon_peer_service::PresentationKey::new(key.tenant().clone(), key.peer().clone(), batch_id.clone());
        let winning_record = trogon_peer_service::PreparedPresentation::from_port(
            batch.clone(),
            record.messages().to_vec(),
            frame.as_str().to_owned(),
        );
        self.inner.insert(&winning_key, &winning_record).await?;
        let plan = trogon_peer::TrustedBatchPlan::from_renderer(
            batch
                .message_ids
                .iter()
                .cloned()
                .map(MessageId::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| PeerPortError::Integrity)?,
            frame.as_str().len(),
        )
        .map_err(|_| PeerPortError::Integrity)?;
        let now = trogonai_proto::convert::datetime_from_timestamp(
            batch.prepared_at.as_option().ok_or(PeerPortError::Integrity)?,
        )
        .map_err(|_| PeerPortError::Integrity)?;
        let command = trogon_peer::PrepareBatch::new(self.owner.clone(), self.attachment.clone(), batch_id, plan, now);
        let principal = trogon_decider_runtime::CommandPrincipal::new(
            trogon_decider_runtime::PrincipalKind::Agent,
            trogon_decider_runtime::PrincipalId::new(self.owner.caller().as_str())
                .map_err(|_| PeerPortError::Integrity)?,
        );
        trogon_decider_runtime::CommandExecution::new(&self.store, &command)
            .with_principal(principal)
            .with_authorizer(&self.policy)
            .with_replay_limit(
                trogon_decider_runtime::ReplayLimit::try_new(4096).map_err(|_| PeerPortError::Integrity)?,
            )
            .execute()
            .await
            .map_err(|_| PeerPortError::Integrity)?;
        Ok(())
    }
}

#[tokio::test]
async fn preparation_race_retains_the_durable_winning_presentation() -> TestResult {
    for mode in [WinningPresentationKey::Candidate, WinningPresentationKey::Distinct] {
        let f = Fixture::new().await?;
        f.intake("cache-race", "winning context").await?;
        let attachment = trogon_peer::AttachmentRef::new(
            PeerId::parse(
                f.b.peer_id
                    .clone()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            )?,
            trogon_peer::ConnectorId::new(
                f.b.connector_id
                    .clone()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            )?,
            f.b.generation.ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let cache = ConcurrentPreparationCache {
            inner: f.cache.clone(),
            store: f.store.clone(),
            policy: f.policy.clone(),
            owner: f.bob.clone(),
            attachment,
            mode,
        };
        let app = trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            cache,
            f.ids.clone(),
            f.catalog.clone(),
        );

        let prepared = app.prepare_batch(&f.bob, f.prepare()).await?;
        assert_eq!(
            prepared.messages.len(),
            1,
            "the real winning command's cache must survive a competing preparation"
        );
        assert_eq!(prepared.messages[0].text.as_deref(), Some("winning context"));
        let batch_id = prepared
            .batch
            .batch_id
            .as_deref()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        let key = format!(
            "{}:{}:{}",
            f.bob.tenant().as_str(),
            f.b.peer_id
                .as_deref()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
            batch_id
        );
        {
            let orphans = f
                .cache
                .orphans
                .lock()
                .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?;
            assert!(!orphans.contains(&key), "a durable winning key cannot be orphaned");
            assert_eq!(
                orphans.len(),
                match mode {
                    WinningPresentationKey::Candidate => 0,
                    WinningPresentationKey::Distinct => 1,
                }
            );
        }
        assert_eq!(f.app().prepare_batch(&f.bob, f.prepare()).await?, prepared);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum LostPreparationReplyError {
    #[error("append committed but its response was lost")]
    Committed,
    #[error("fixture append failed")]
    Inner(#[source] <StoreFixture as trogon_decider_runtime::StreamAppend<PeerId>>::Error),
}

struct CommitThenLoseReplyStore {
    inner: StoreFixture,
    lose_reply: std::sync::atomic::AtomicBool,
}

impl trogon_peer_service::ScopedPeerStore for CommitThenLoseReplyStore {
    fn tenant(&self) -> &trogon_peer::TenantId {
        &self.inner.tenant
    }
}

impl trogon_decider_runtime::StreamRead<PeerId> for CommitThenLoseReplyStore {
    type Error = <StoreFixture as trogon_decider_runtime::StreamRead<PeerId>>::Error;
    async fn read_stream(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, PeerId>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamRead::read_stream(&self.inner, request).await
    }
    async fn read_stream_bounded(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, PeerId>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamRead::read_stream_bounded(&self.inner, request, maximum).await
    }
}

impl trogon_decider_runtime::StreamAppend<PeerId> for CommitThenLoseReplyStore {
    type Error = LostPreparationReplyError;
    async fn append_stream(
        &self,
        request: trogon_decider_runtime::AppendStreamRequest<'_, PeerId>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        let result = trogon_decider_runtime::StreamAppend::append_stream(&self.inner, request)
            .await
            .map_err(LostPreparationReplyError::Inner)?;
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            return Err(LostPreparationReplyError::Committed);
        }
        Ok(result)
    }
    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        match error {
            LostPreparationReplyError::Committed => trogon_decider_runtime::AppendFailure::Fatal,
            LostPreparationReplyError::Inner(error) => {
                trogon_decider_runtime::StreamAppend::classify_append_failure(&self.inner, error)
            }
        }
    }
}

#[tokio::test]
async fn uncertain_preparation_append_retains_context_for_durable_retry() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("lost-prepare-reply", "durably prepared context").await?;
    let app = trogon_peer_service::PeerApplication::new(
        CommitThenLoseReplyStore {
            inner: f.store.clone(),
            lose_reply: std::sync::atomic::AtomicBool::new(true),
        },
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    assert!(
        app.prepare_batch(&f.bob, f.prepare()).await.is_err(),
        "lost reply must remain an uncertain operation error"
    );
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .is_empty(),
        "an append error cannot establish that the durable candidate lost"
    );
    let replay = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(replay.messages.len(), 1);
    assert_eq!(replay.messages[0].text.as_deref(), Some("durably prepared context"));
    let before = f.store.appends.load(Ordering::SeqCst);
    assert_eq!(f.app().prepare_batch(&f.bob, f.prepare()).await?, replay);
    assert_eq!(
        f.store.appends.load(Ordering::SeqCst),
        before,
        "read retry must not append a second preparation"
    );
    Ok(())
}

#[test]
fn prompt_correlation_preserves_a_valid_128_byte_host_identity() -> TestResult {
    let correlation = "é".repeat(64);
    assert_eq!(
        trogon_peer_service::PromptId::new(correlation.clone())?.as_str(),
        correlation
    );
    Ok(())
}

#[tokio::test]
async fn missing_oldest_preparation_content_leaves_the_assignment_uncommitted() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("oldest", "first").await?;
    f.intake("later", "second").await?;
    f.content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .clear();
    let appends = f.store.appends.load(Ordering::SeqCst);
    let result = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert!(!result.batch.is_set());
    assert!(result.messages.is_empty());
    assert_eq!(
        result.content_availability.and_then(|v| v.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE)
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn missing_later_content_commits_only_the_renderable_oldest_prefix() -> TestResult {
    let f = Fixture::new().await?;
    let first = f.intake("oldest", "first").await?;
    let second = f.intake("later", "second").await?;
    let reference = second
        .message
        .content_ref
        .as_option()
        .and_then(|claim| claim.storage_ref.as_option())
        .and_then(|locator| locator.value.as_deref())
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let saved = f
        .content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .remove(reference)
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].metadata, first.message);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    f.content
        .bodies
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .insert(reference.to_owned(), saved);
    assert_eq!(
        f.app().prepare_batch(&f.bob, f.prepare()).await?,
        prepared,
        "restored later content cannot rewrite the durable assignment"
    );
    Ok(())
}

struct ReturnedOpenText(trogon_peer_service::MessageText);

impl trogon_peer_service::ContentOpen for ReturnedOpenText {
    async fn open(
        &self,
        _scope: &trogon_peer_service::PeerAccess,
        _binding: &trogon_peer_service::ContentBinding,
    ) -> Result<Option<trogon_peer_service::MessageText>, trogon_peer_service::PeerPortError> {
        Ok(Some(self.0.clone()))
    }
}

#[tokio::test]
async fn incorrect_or_unrenderable_opened_content_cannot_commit_a_preparation() -> TestResult {
    for (accepted, returned, expect_context) in [
        ("payload".to_owned(), "short".to_owned(), false),
        ("a".repeat(2048), "<".repeat(2048), true),
    ] {
        let f = Fixture::new().await?;
        f.intake("untrusted-open", &accepted).await?;
        let app = trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            ReturnedOpenText(trogon_peer_service::MessageText::new(returned)?),
            f.cache.clone(),
            f.ids.clone(),
            f.catalog.clone(),
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        let result = app.prepare_batch(&f.bob, f.prepare()).await;
        if expect_context {
            assert!(matches!(
                result,
                Err(trogon_peer_service::PeerApplicationError::Context(_))
            ));
        } else {
            assert!(matches!(result, Err(trogon_peer_service::PeerApplicationError::Scope)));
        }
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum PresentationReadFailure {
    Unavailable,
    Integrity,
}

struct FailingPresentationRead {
    inner: PresentationFixture,
    failure: PresentationReadFailure,
}

impl trogon_peer_service::PresentationRead for FailingPresentationRead {
    async fn read(
        &self,
        _key: &trogon_peer_service::PresentationKey,
    ) -> Result<Option<trogon_peer_service::PreparedPresentation>, trogon_peer_service::PeerPortError> {
        match self.failure {
            PresentationReadFailure::Unavailable => Err(trogon_peer_service::PeerPortError::Unavailable),
            PresentationReadFailure::Integrity => Err(trogon_peer_service::PeerPortError::Integrity),
        }
    }
}

impl trogon_peer_service::PresentationInsert for FailingPresentationRead {
    async fn insert(
        &self,
        key: &trogon_peer_service::PresentationKey,
        record: &trogon_peer_service::PreparedPresentation,
    ) -> Result<(), trogon_peer_service::PeerPortError> {
        trogon_peer_service::PresentationInsert::insert(&self.inner, key, record).await
    }
}

impl trogon_peer_service::PresentationOrphan for FailingPresentationRead {
    async fn orphan(
        &self,
        key: &trogon_peer_service::PresentationKey,
    ) -> Result<(), trogon_peer_service::PeerPortError> {
        trogon_peer_service::PresentationOrphan::orphan(&self.inner, key).await
    }
}

#[tokio::test]
async fn a_prepared_batch_retains_identity_when_cache_is_unavailable_and_surfaces_integrity_failure() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("cached", "exact context").await?;
    let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    for failure in [PresentationReadFailure::Unavailable, PresentationReadFailure::Integrity] {
        let app = trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            FailingPresentationRead {
                inner: f.cache.clone(),
                failure,
            },
            f.ids.clone(),
            f.catalog.clone(),
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        let result = app.prepare_batch(&f.bob, f.prepare()).await;
        if matches!(failure, PresentationReadFailure::Integrity) {
            assert!(matches!(
                result,
                Err(trogon_peer_service::PeerApplicationError::Port(
                    trogon_peer_service::PeerPortError::Integrity
                ))
            ));
        } else {
            let unavailable = result?;
            assert_eq!(unavailable.batch, prepared.batch);
            assert!(unavailable.messages.is_empty());
            assert_eq!(
                unavailable.content_availability.and_then(|v| v.as_known()),
                Some(proto::ContentAvailability::CONTENT_AVAILABILITY_RETRY_UNAVAILABLE)
            );
        }
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    }
    assert_eq!(f.app().prepare_batch(&f.bob, f.prepare()).await?, prepared);
    Ok(())
}

struct ReplaceAttachmentDuringOpen<'a>(&'a Fixture);

impl trogon_peer_service::ContentOpen for ReplaceAttachmentDuringOpen<'_> {
    async fn open(
        &self,
        scope: &trogon_peer_service::PeerAccess,
        binding: &trogon_peer_service::ContentBinding,
    ) -> Result<Option<trogon_peer_service::MessageText>, trogon_peer_service::PeerPortError> {
        let f = self.0;
        let service = trogon_peer_service::PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
        service
            .detach(
                &f.bob,
                proto::DetachRequest {
                    attachment_ref: f.b.clone().into(),
                },
            )
            .await
            .map_err(|error| trogon_peer_service::PeerPortError::Backend(Box::new(error)))?;
        service
            .enroll(&f.bob, Fixture::enrollment("bob"))
            .await
            .map_err(|error| trogon_peer_service::PeerPortError::Backend(Box::new(error)))?;
        trogon_peer_service::ContentOpen::open(&f.content, scope, binding).await
    }
}

#[tokio::test]
async fn preparation_fenced_during_content_open_orphans_only_its_rejected_candidate() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("replacement", "pending context").await?;
    let app = trogon_peer_service::PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        ReplaceAttachmentDuringOpen(&f),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    assert!(matches!(
        app.prepare_batch(&f.bob, f.prepare()).await,
        Err(trogon_peer_service::PeerApplicationError::Message(
            trogon_decider_runtime::CommandError::Decide(trogon_peer::MessageError::Guard(
                trogon_peer::HeartbeatError::Fenced
            ))
        ))
    ));
    assert_eq!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .len(),
        1
    );
    let service = trogon_peer_service::PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
    let replacement = service.enroll(&f.bob, Fixture::enrollment("bob")).await?;
    assert_eq!(replacement.attachment.attachment_ref.generation, Some(2));
    let mut request = f.prepare();
    request.attachment_ref = replacement.attachment.attachment_ref.clone();
    let prepared = f.app().prepare_batch(&f.bob, request).await?;
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].text.as_deref(), Some("pending context"));
    let winning_id = prepared
        .batch
        .batch_id
        .as_deref()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let winner_key = format!(
        "{}:{}:{}",
        f.bob.tenant().as_str(),
        f.b.peer_id
            .as_deref()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        winning_id
    );
    assert!(
        !f.cache
            .orphans
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .contains(&winner_key)
    );
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
        attachment_ref: f.b.clone().into(),
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
                attachment_ref: f.a.clone().into(),
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
async fn acknowledging_an_unknown_batch_cannot_release_capacity_or_append_a_receipt() -> TestResult {
    let f = Fixture::new().await?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .acknowledge_batch(
                &f.bob,
                proto::AcknowledgeBatchRequest {
                    attachment_ref: f.b.clone().into(),
                    batch_id: Some("unknown-batch".into()),
                }
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Message(
            trogon_decider_runtime::CommandError::Decide(trogon_peer::MessageError::BatchNotFound)
        ))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert!(!f.app().prepare_batch(&f.bob, f.prepare()).await?.batch.is_set());
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
                attachment_ref: f.a.clone().into(),
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
                attachment_ref: f.a.clone().into(),
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
                    attachment_ref: f.a.clone().into(),
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
async fn oversized_roster_requests_fail_before_storage_and_catalog_access() -> TestResult {
    let f = Fixture::new().await?;
    let reads = f.store.reads.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .roster(
                &f.alice,
                proto::RosterRequest {
                    attachment_ref: f.a.clone().into(),
                    page_size: Some(65),
                    ..Default::default()
                }
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Port(
            trogon_peer_service::PeerPortError::Denied
        ))
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

struct ReturnedCatalogPage(Vec<trogon_peer::PeerId>);

impl trogon_peer_service::PeerCatalog for ReturnedCatalogPage {
    async fn page(
        &self,
        _scope: &trogon_peer_service::PeerAccess,
        _collaboration: &trogon_peer::CollaborationRef,
        _after: Option<&trogon_peer::PeerId>,
        _limit: trogon_peer_service::WorkLimit,
    ) -> Result<Vec<trogon_peer::PeerId>, trogon_peer_service::PeerPortError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn roster_rejects_overfull_duplicate_unsorted_and_rewound_catalog_pages() -> TestResult {
    let f = Fixture::new().await?;
    let mut ids = [
        trogon_peer::PeerId::parse(
            f.a.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?,
        trogon_peer::PeerId::parse(
            f.b.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?,
    ];
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for (members, page_size, page_token) in [
        (ids.to_vec(), 1, None),
        (vec![ids[0].clone(), ids[0].clone()], 2, None),
        (vec![ids[1].clone(), ids[0].clone()], 2, None),
        (vec![ids[0].clone()], 2, Some(ids[1].as_str().to_owned())),
    ] {
        let app = trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            f.ids.clone(),
            ReturnedCatalogPage(members),
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        assert!(matches!(
            app.roster(
                &f.alice,
                proto::RosterRequest {
                    attachment_ref: f.a.clone().into(),
                    page_size: Some(page_size),
                    page_token,
                }
            )
            .await,
            Err(trogon_peer_service::PeerApplicationError::Scope)
        ));
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn catalog_membership_cannot_advertise_a_peer_from_another_collaboration() -> TestResult {
    let f = Fixture::new().await?;
    let isolated_owner =
        trogon_peer::TrustedPeerOwner::new(f.alice.tenant().clone(), trogon_peer::CallerId::new("isolated")?);
    let service = trogon_peer_service::PeerService::new(
        f.store.clone(),
        ServiceCoverageIsolatedEnrollment(isolated_owner.clone()),
        f.clock.clone(),
    );
    let mut enrollment = Fixture::enrollment("isolated");
    enrollment.collaboration_ref = Some("isolated".into());
    let isolated = service.enroll(&isolated_owner, enrollment).await?;
    let id = trogon_peer::PeerId::parse(isolated.peer_id.ok_or(trogon_peer_service::PeerPortError::Integrity)?)?;
    f.policy
        .grants
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .insert(id.as_str().to_owned(), isolated_owner);
    f.catalog
        .peers
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .push(id);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        f.app()
            .roster(
                &f.alice,
                proto::RosterRequest {
                    attachment_ref: f.a.clone().into(),
                    ..Default::default()
                }
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
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
async fn contending_preparers_with_the_same_cache_key_never_orphan_the_winner() -> TestResult {
    let f = Fixture::new().await?;
    f.intake("shared_key", "retain the winning presentation").await?;
    let ids = fixed_batch_id::FixedBatchId(trogon_peer::BatchId::new("shared_batch")?);
    let app = || {
        trogon_peer_service::PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            ids.clone(),
            f.catalog.clone(),
        )
    };
    *f.store
        .append_barrier
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? =
        Some(std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    let appends = f.store.appends.load(Ordering::SeqCst);
    let left = app();
    let right = app();
    let (left, right) = tokio::join!(
        left.prepare_batch(&f.bob, f.prepare()),
        right.prepare_batch(&f.bob, f.prepare())
    );
    let left = left?;
    let right = right?;
    assert_eq!(left, right);
    assert_eq!(left.batch.batch_id.as_deref(), Some("shared_batch"));
    assert_eq!(
        left.messages[0].text.as_deref(),
        Some("retain the winning presentation")
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 2);
    assert_eq!(
        f.cache
            .records
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .len(),
        1
    );
    let recipient = PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    assert_eq!(f.history(&recipient).await?.batches.len(), 1);
    assert_eq!(app().prepare_batch(&f.bob, f.prepare()).await?, left);
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .is_empty(),
        "the durable winner's shared cache key must not be marked orphan"
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
                    attachment_ref: f.a.clone().into(),
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
                    attachment_ref: f.b.clone().into(),
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
            request.attachment_ref = reference.clone().into();
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
                attachment_ref: f.b.clone().into(),
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
                attachment_ref: f.b.clone().into(),
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
        .attachment_ref
        .clone()
        .into_option()
        .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
    let roster = f
        .app()
        .roster(
            &f.alice,
            proto::RosterRequest {
                attachment_ref: f.a.clone().into(),
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
                attachment_ref: f.a.clone().into(),
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
                    attachment_ref: reference.clone().into(),
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
                attachment_ref: c.clone().into(),
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
        .attachment_ref
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
                    attachment_ref: f.a.clone().into(),
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
                attachment_ref: f.a.clone().into(),
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
                    attachment_ref: f.a.clone().into(),
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
                    attachment_ref: f.a.clone().into()
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

#[derive(Clone, Copy)]
enum MalformedHistoryField {
    MissingCollaboration,
    EmptyCollaboration,
    ControlCollaboration,
    OversizedCollaboration,
    MissingRecipient,
    InvalidRecipient,
    MissingContentRef,
    MissingContentLocator,
    InvalidContentLocator,
    MissingContentDigest,
    InvalidContentDigestAlgorithm,
    ShortContentDigest,
    MissingContentSize,
    ZeroContentSize,
    OversizedContentSize,
    MissingSemanticFingerprint,
    MissingSemanticFingerprintValue,
    ShortSemanticFingerprint,
}

impl MalformedHistoryField {
    fn expected(self) -> trogon_peer::EvolveError {
        match self {
            Self::MissingCollaboration => EvolveError::MissingField {
                field: "enrollment.collaboration_ref",
            },
            Self::EmptyCollaboration => EvolveError::InvalidValue {
                field: "enrollment.collaboration_ref",
                source: PeerValueError::Empty,
            },
            Self::ControlCollaboration => EvolveError::InvalidValue {
                field: "enrollment.collaboration_ref",
                source: PeerValueError::InvalidCharacter,
            },
            Self::OversizedCollaboration => EvolveError::InvalidValue {
                field: "enrollment.collaboration_ref",
                source: PeerValueError::TooLong { maximum: 256 },
            },
            Self::MissingRecipient => EvolveError::MissingField {
                field: "recipient_peer_id",
            },
            Self::InvalidRecipient => EvolveError::InvalidValue {
                field: "recipient_peer_id",
                source: PeerValueError::InvalidPeerId,
            },
            Self::MissingContentRef => EvolveError::MissingField { field: "content_ref" },
            Self::MissingContentLocator
            | Self::MissingContentDigest
            | Self::InvalidContentDigestAlgorithm
            | Self::ShortContentDigest => EvolveError::InvalidValue {
                field: "content_ref",
                source: PeerValueError::InvalidContentReference,
            },
            Self::InvalidContentLocator => EvolveError::InvalidValue {
                field: "content_ref",
                source: PeerValueError::InvalidCharacter,
            },
            Self::MissingContentSize => EvolveError::MissingField {
                field: "content_size_bytes",
            },
            Self::ZeroContentSize | Self::OversizedContentSize => EvolveError::InvalidValue {
                field: "content",
                source: PeerValueError::InvalidContent,
            },
            Self::MissingSemanticFingerprint => EvolveError::MissingField {
                field: "semantic_fingerprint",
            },
            Self::MissingSemanticFingerprintValue => EvolveError::MissingField {
                field: "semantic_fingerprint.value",
            },
            Self::ShortSemanticFingerprint => EvolveError::InvalidValue {
                field: "semantic_fingerprint",
                source: PeerValueError::InvalidSemanticFingerprint,
            },
        }
    }
}

struct AlteredHistoryEnvelope {
    inner: StoreFixture,
    peer: trogon_peer::PeerId,
    event_id: trogon_decider_runtime::EventId,
    content: Vec<u8>,
}

impl AlteredHistoryEnvelope {
    fn alter(&self, peer: &trogon_peer::PeerId, response: &mut trogon_decider_runtime::ReadStreamResponse) {
        if peer == &self.peer {
            for event in &mut response.events {
                if event.event.id == self.event_id {
                    event.event.content = self.content.clone();
                }
            }
        }
    }
}

impl trogon_peer_service::ScopedPeerStore for AlteredHistoryEnvelope {
    fn tenant(&self) -> &trogon_peer::TenantId {
        &self.inner.tenant
    }
}

impl trogon_decider_runtime::StreamRead<trogon_peer::PeerId> for AlteredHistoryEnvelope {
    type Error = <StoreFixture as trogon_decider_runtime::StreamRead<trogon_peer::PeerId>>::Error;

    async fn read_stream(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        let peer = request.stream_id.clone();
        let mut response = trogon_decider_runtime::StreamRead::read_stream(&self.inner, request).await?;
        self.alter(&peer, &mut response);
        Ok(response)
    }

    async fn read_stream_bounded(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, trogon_peer::PeerId>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        let peer = request.stream_id.clone();
        let mut response =
            trogon_decider_runtime::StreamRead::read_stream_bounded(&self.inner, request, maximum).await?;
        self.alter(&peer, &mut response);
        Ok(response)
    }
}

impl trogon_decider_runtime::StreamAppend<trogon_peer::PeerId> for AlteredHistoryEnvelope {
    type Error = <StoreFixture as trogon_decider_runtime::StreamAppend<trogon_peer::PeerId>>::Error;

    async fn append_stream(
        &self,
        request: trogon_decider_runtime::AppendStreamRequest<'_, trogon_peer::PeerId>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamAppend::append_stream(&self.inner, request).await
    }

    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        trogon_decider_runtime::StreamAppend::classify_append_failure(&self.inner, error)
    }
}

async fn altered_history_store(
    fixture: &Fixture,
    peer: trogon_peer::PeerId,
    field: MalformedHistoryField,
) -> TestResult<AlteredHistoryEnvelope> {
    let history = fixture
        .store
        .read_stream(ReadStreamRequest {
            stream_id: &peer,
            from: ReadFrom::Beginning,
        })
        .await?;
    for recorded in history.events {
        let mut decoded = recorded
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
        let changed = match (field, decoded.event.as_mut()) {
            (
                MalformedHistoryField::MissingCollaboration
                | MalformedHistoryField::EmptyCollaboration
                | MalformedHistoryField::ControlCollaboration
                | MalformedHistoryField::OversizedCollaboration,
                Some(PeerEventCase::PeerEnrolled(event)),
            ) => {
                event
                    .enrollment
                    .as_option_mut()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                    .collaboration_ref = match field {
                    MalformedHistoryField::MissingCollaboration => None,
                    MalformedHistoryField::EmptyCollaboration => Some(String::new()),
                    MalformedHistoryField::ControlCollaboration => Some("shared\ninvalid".into()),
                    MalformedHistoryField::OversizedCollaboration => Some("x".repeat(257)),
                    _ => return Err(trogon_peer_service::PeerPortError::Integrity.into()),
                };
                true
            }
            (
                MalformedHistoryField::MissingRecipient | MalformedHistoryField::InvalidRecipient,
                Some(PeerEventCase::PeerMessageAccepted(event)),
            ) => {
                event
                    .entry
                    .as_option_mut()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                    .message
                    .as_option_mut()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                    .recipient_peer_id = match field {
                    MalformedHistoryField::MissingRecipient => None,
                    MalformedHistoryField::InvalidRecipient => Some("not-a-peer".into()),
                    _ => return Err(trogon_peer_service::PeerPortError::Integrity.into()),
                };
                true
            }
            (
                MalformedHistoryField::MissingContentRef
                | MalformedHistoryField::MissingContentLocator
                | MalformedHistoryField::InvalidContentLocator
                | MalformedHistoryField::MissingContentDigest
                | MalformedHistoryField::InvalidContentDigestAlgorithm
                | MalformedHistoryField::ShortContentDigest
                | MalformedHistoryField::MissingContentSize
                | MalformedHistoryField::ZeroContentSize
                | MalformedHistoryField::OversizedContentSize
                | MalformedHistoryField::MissingSemanticFingerprint
                | MalformedHistoryField::MissingSemanticFingerprintValue
                | MalformedHistoryField::ShortSemanticFingerprint,
                Some(PeerEventCase::PeerMessageAccepted(event)),
            ) => {
                let message = event
                    .entry
                    .as_option_mut()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                    .message
                    .as_option_mut()
                    .ok_or(trogon_peer_service::PeerPortError::Integrity)?;
                match field {
                    MalformedHistoryField::MissingContentRef => message.content_ref = Default::default(),
                    MalformedHistoryField::MissingContentLocator => {
                        message
                            .content_ref
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .storage_ref = Default::default();
                    }
                    MalformedHistoryField::InvalidContentLocator => {
                        message
                            .content_ref
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .storage_ref
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .value = Some("invalid/location".into());
                    }
                    MalformedHistoryField::MissingContentDigest => {
                        message
                            .content_ref
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .digest = Default::default();
                    }
                    MalformedHistoryField::InvalidContentDigestAlgorithm => {
                        message
                            .content_ref
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .digest
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .algorithm = Some("sha512".into());
                    }
                    MalformedHistoryField::ShortContentDigest => {
                        message
                            .content_ref
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .digest
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .value
                            .as_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .truncate(31);
                    }
                    MalformedHistoryField::MissingContentSize => message.content_size_bytes = None,
                    MalformedHistoryField::ZeroContentSize => message.content_size_bytes = Some(0),
                    MalformedHistoryField::OversizedContentSize => message.content_size_bytes = Some(2049),
                    MalformedHistoryField::MissingSemanticFingerprint => {
                        message.semantic_fingerprint = Default::default();
                    }
                    MalformedHistoryField::MissingSemanticFingerprintValue => {
                        message
                            .semantic_fingerprint
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .value = None;
                    }
                    MalformedHistoryField::ShortSemanticFingerprint => {
                        message
                            .semantic_fingerprint
                            .as_option_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .value
                            .as_mut()
                            .ok_or(trogon_peer_service::PeerPortError::Integrity)?
                            .truncate(31);
                    }
                    _ => return Err(trogon_peer_service::PeerPortError::Integrity.into()),
                }
                true
            }
            _ => false,
        };
        if changed {
            return Ok(AlteredHistoryEnvelope {
                inner: fixture.store.clone(),
                peer,
                event_id: recorded.event.id,
                content: decoded.encode()?,
            });
        }
    }
    Err(trogon_peer_service::PeerPortError::Integrity.into())
}

#[tokio::test]
async fn malformed_enrollment_history_cannot_reach_send_or_status_as_an_application_value_error() -> TestResult {
    for field in [
        MalformedHistoryField::MissingCollaboration,
        MalformedHistoryField::EmptyCollaboration,
        MalformedHistoryField::ControlCollaboration,
        MalformedHistoryField::OversizedCollaboration,
    ] {
        let f = Fixture::new().await?;
        let peer = trogon_peer::PeerId::parse(
            f.a.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let store = altered_history_store(&f, peer, field).await?;
        let app = trogon_peer_service::PeerApplication::new(
            store,
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            f.ids.clone(),
            f.catalog.clone(),
        );
        let reads = f.store.reads.load(Ordering::SeqCst);
        let appends = f.store.appends.load(Ordering::SeqCst);
        let expected = field.expected();
        assert!(matches!(
            app.send(&f.alice, f.send("bad-collaboration", "private")).await,
            Err(trogon_peer_service::PeerApplicationError::History(
                trogon_peer_service::HistoryError::Evolve(ref source)
            )) if source == &expected
        ));
        assert!(matches!(
            app.status(&f.alice, proto::StatusRequest {
                attachment_ref: f.a.clone().into(),
                idempotency_key: Some("not-an-entry".into()),
            }).await,
            Err(trogon_peer_service::PeerApplicationError::History(
                trogon_peer_service::HistoryError::Evolve(ref source)
            )) if source == &expected
        ));
        assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 2);
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn malformed_accepted_recipient_history_fails_replay_before_recipient_authority_or_ports() -> TestResult {
    for field in [
        MalformedHistoryField::MissingRecipient,
        MalformedHistoryField::InvalidRecipient,
    ] {
        let f = Fixture::new().await?;
        f.app().send(&f.alice, f.send("recorded", "private")).await?;
        let sender = trogon_peer::PeerId::parse(
            f.a.peer_id
                .clone()
                .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
        )?;
        let store = altered_history_store(&f, sender, field).await?;
        let app = trogon_peer_service::PeerApplication::new(
            store,
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            f.ids.clone(),
            f.catalog.clone(),
        );
        let reads = f.store.reads.load(Ordering::SeqCst);
        let appends = f.store.appends.load(Ordering::SeqCst);
        let content = f.content.calls.load(Ordering::SeqCst);
        let expected = field.expected();
        assert!(matches!(
            app.status(&f.alice, proto::StatusRequest {
                attachment_ref: f.a.clone().into(),
                idempotency_key: Some("recorded".into()),
            }).await,
            Err(trogon_peer_service::PeerApplicationError::History(
                trogon_peer_service::HistoryError::Evolve(ref source)
            )) if source == &expected
        ));
        assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 1);
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), content);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_granted_peer_without_enrollment_history_cannot_authorize_status_or_receive_a_send() -> TestResult {
    let f = Fixture::new().await?;
    let unknown = trogon_peer::PeerId::parse("0".repeat(64))?;
    assert_ne!(f.a.peer_id.as_deref(), Some(unknown.as_str()));
    assert_ne!(f.b.peer_id.as_deref(), Some(unknown.as_str()));
    assert!(
        f.policy
            .grants
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
            .insert(unknown.as_str().to_owned(), f.alice.clone())
            .is_none()
    );
    let reads = f.store.reads.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    let mut reference = f.a.clone();
    reference.peer_id = Some(unknown.as_str().to_owned());
    assert!(matches!(
        f.app()
            .status(
                &f.alice,
                proto::StatusRequest {
                    attachment_ref: reference.into(),
                    idempotency_key: Some("unknown-history".into()),
                },
            )
            .await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 1);
    let mut request = f.send("unknown-recipient", "private");
    request.recipient_peer_id = Some(unknown.as_str().to_owned());
    assert!(matches!(
        f.app().send(&f.alice, request).await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 3);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_cross_caller_reconcile_grant_cannot_settle_another_callers_obligations() -> TestResult {
    let f = Fixture::new().await?;
    let other = trogon_peer::PeerId::parse(
        f.b.peer_id
            .clone()
            .ok_or(trogon_peer_service::PeerPortError::Integrity)?,
    )?;
    f.policy
        .grants
        .lock()
        .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)?
        .insert(other.as_str().to_owned(), f.alice.clone());
    let before = (
        f.store.reads.load(Ordering::SeqCst),
        f.store.appends.load(Ordering::SeqCst),
    );
    let id = trogon_peer::MessageId::new("another_callers_obligation")?;
    assert!(matches!(
        f.app()
            .reconcile(&f.alice, &other, &[id], trogon_peer_service::WorkLimit::new(1)?)
            .await,
        Err(trogon_peer_service::PeerApplicationError::Scope)
    ));
    assert_eq!(f.store.reads.load(Ordering::SeqCst), before.0 + 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before.1);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn an_already_expired_obligation_is_disposed_without_opening_content() -> TestResult {
    let mut f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("expired-before-open", "pending")).await?;
    let id = MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let sender = PeerId::parse(sent.message.sender_peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let recipient = PeerId::parse(sent.message.recipient_peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = trogonai_proto::convert::datetime_from_timestamp(
        sent.message.expires_at.as_option().ok_or(PeerPortError::Integrity)?,
    )?;
    f.a = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .enroll(&f.alice, Fixture::enrollment("alice"))
        .await?
        .attachment
        .attachment_ref
        .clone()
        .into_option()
        .ok_or(PeerPortError::Integrity)?;
    assert_eq!(f.a.peer_id.as_deref(), Some(sender.as_str()));
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert_eq!(
        f.app()
            .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await?,
        1
    );
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 2);
    let target = service_coverage_public_history(&f, &recipient).await?;
    assert!(target.inbox.is_empty());
    assert_eq!(target.dispositions.len(), 1);
    let receipt = target.dispositions.first().ok_or(PeerPortError::Integrity)?;
    assert_eq!(receipt.message_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        receipt.outcome.and_then(|value| value.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED)
    );
    assert_eq!(receipt.decided_at, sent.message.expires_at);
    let source = service_coverage_public_history(&f, &sender).await?;
    assert_eq!(
        source
            .outbox
            .first()
            .ok_or(PeerPortError::Integrity)?
            .settlement
            .as_option(),
        Some(receipt)
    );
    let status = f
        .app()
        .status(
            &f.alice,
            proto::StatusRequest {
                attachment_ref: f.a.clone().into(),
                idempotency_key: Some("expired-before-open".into()),
            },
        )
        .await?;
    assert_eq!(
        status.status.and_then(|value| value.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_EXPIRED)
    );
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

struct WrongSizeReconciliationContent(DisposableContentFixture);

impl trogon_peer_service::ContentOpen for WrongSizeReconciliationContent {
    async fn open(
        &self,
        scope: &trogon_peer_service::PeerAccess,
        binding: &trogon_peer_service::ContentBinding,
    ) -> Result<Option<trogon_peer_service::MessageText>, trogon_peer_service::PeerPortError> {
        match ContentOpen::open(&self.0, scope, binding).await? {
            Some(text) => Ok(Some(
                MessageText::new(format!("{}!", text.as_str())).map_err(|_| PeerPortError::Integrity)?,
            )),
            None => Ok(None),
        }
    }
}

#[tokio::test]
async fn a_wrong_size_content_response_cannot_intake_or_dispose_an_obligation() -> TestResult {
    let f = Fixture::new().await?;
    let sent = f.app().send(&f.alice, f.send("wrong-open-size", "pending")).await?;
    let id = MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let sender = PeerId::parse(sent.message.sender_peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let recipient = PeerId::parse(sent.message.recipient_peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let app = PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        WrongSizeReconciliationContent(f.content.clone()),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    assert!(matches!(
        app.reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await,
        Err(PeerApplicationError::Scope)
    ));
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls + 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    let target = service_coverage_public_history(&f, &recipient).await?;
    assert!(target.inbox.is_empty());
    assert!(target.dispositions.is_empty());
    let source = service_coverage_public_history(&f, &sender).await?;
    assert!(
        !source
            .outbox
            .first()
            .ok_or(PeerPortError::Integrity)?
            .settlement
            .is_set()
    );
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert!(
        f.cache
            .records
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.app()
            .reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
            .await?,
        1
    );
    let target = service_coverage_public_history(&f, &recipient).await?;
    assert_eq!(target.inbox.len(), 1);
    assert_eq!(target.inbox[0].message, sent.message);
    assert!(target.dispositions.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_full_recipient_defers_only_its_obligation_and_retries_after_capacity_is_released() -> TestResult {
    let f = Fixture::new().await?;
    let recipient = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let service = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone());
    let mut first_obligation = None;
    for sender_number in 0..8 {
        let reference = service
            .enroll(&f.alice, Fixture::enrollment(&format!("quota_sender_{sender_number}")))
            .await?
            .attachment
            .attachment_ref
            .clone()
            .into_option()
            .ok_or(PeerPortError::Integrity)?;
        let sender = PeerId::parse(reference.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
        assert!(
            f.policy
                .grants
                .lock()
                .map_err(|_| PeerPortError::Unavailable)?
                .insert(sender.as_str().to_owned(), f.alice.clone())
                .is_none()
        );
        let mut ids = Vec::new();
        for message_number in 0..8 {
            let mut request = f.send(&format!("quota_{sender_number}_{message_number}"), "queued");
            request.attachment_ref = reference.clone().into();
            let sent = f.app().send(&f.alice, request).await?;
            let id = MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
            if first_obligation.is_none() {
                first_obligation = Some((
                    sender.clone(),
                    id.clone(),
                    sent.message
                        .content_ref
                        .storage_ref
                        .value
                        .clone()
                        .ok_or(PeerPortError::Integrity)?,
                ));
            }
            ids.push(id);
        }
        assert_eq!(f.app().reconcile(&f.alice, &sender, &ids, WorkLimit::new(8)?).await?, 8);
    }
    let full = service_coverage_public_history(&f, &recipient).await?;
    assert_eq!(full.inbox.len(), 64);
    assert!(full.inbox.iter().all(|entry| !entry.disposition.is_set()));
    assert!(full.dispositions.is_empty());
    let available = service
        .enroll(&f.bob, Fixture::enrollment("available_recipient"))
        .await?
        .attachment
        .attachment_ref
        .clone()
        .into_option()
        .ok_or(PeerPortError::Integrity)?;
    let available = PeerId::parse(available.peer_id.ok_or(PeerPortError::Integrity)?)?;
    assert!(
        f.policy
            .grants
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .insert(available.as_str().to_owned(), f.bob.clone())
            .is_none()
    );
    let blocked = f.app().send(&f.alice, f.send("capacity_blocked", "pending")).await?;
    let blocked_id = MessageId::new(blocked.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let mut next_request = f.send("capacity_independent", "next");
    next_request.recipient_peer_id = Some(available.as_str().to_owned());
    let next = f.app().send(&f.alice, next_request).await?;
    let next_id = MessageId::new(next.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let sender = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let source = service_coverage_public_history(&f, &sender).await?;
    let evidence = AcceptedMessage::from_sender_history(&source, &blocked_id)?;
    let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(f.alice.caller().as_str())?);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        CommandExecution::new(&f.store, &IntakeMessage::new(evidence, f.clock.now()))
            .with_principal(principal)
            .with_authorizer(&f.policy)
            .with_replay_limit(ReplayLimit::try_new(4096)?)
            .execute()
            .await,
        Err(CommandError::Decide(MessageError::DeferIntake))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    assert_eq!(
        f.app()
            .reconcile(
                &f.alice,
                &sender,
                &[blocked_id.clone(), next_id.clone()],
                WorkLimit::new(2)?
            )
            .await?,
        2
    );
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls + 2);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    let target = service_coverage_public_history(&f, &recipient).await?;
    assert_eq!(target.inbox, full.inbox);
    assert!(target.dispositions.is_empty());
    let target = service_coverage_public_history(&f, &available).await?;
    assert_eq!(target.inbox.len(), 1);
    assert_eq!(target.inbox[0].message, next.message);
    assert!(target.dispositions.is_empty());
    let source = service_coverage_public_history(&f, &sender).await?;
    assert_eq!(source.outbox.len(), 2);
    assert!(source.outbox.iter().all(|entry| !entry.settlement.is_set()));
    let (first_sender, first_id, first_ref) = first_obligation.ok_or(PeerPortError::Integrity)?;
    assert!(
        f.content
            .bodies
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .remove(&first_ref)
            .is_some()
    );
    assert_eq!(
        f.app()
            .reconcile(&f.alice, &first_sender, &[first_id], WorkLimit::new(1)?)
            .await?,
        1
    );
    assert_eq!(
        f.app()
            .reconcile(&f.alice, &sender, std::slice::from_ref(&blocked_id), WorkLimit::new(1)?)
            .await?,
        1
    );
    let target = service_coverage_public_history(&f, &recipient).await?;
    assert_eq!(target.inbox.len(), 65);
    assert_eq!(
        target.inbox.iter().filter(|entry| !entry.disposition.is_set()).count(),
        64
    );
    assert_eq!(target.dispositions.len(), 1);
    assert_eq!(
        target
            .inbox
            .iter()
            .filter(|entry| entry.message.message_id.as_deref() == Some(blocked_id.as_str()))
            .count(),
        1
    );
    let source = service_coverage_public_history(&f, &sender).await?;
    assert!(source.outbox.iter().all(|entry| !entry.settlement.is_set()));
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn malformed_accepted_content_history_fails_before_status_or_reconciliation_ports() -> TestResult {
    for field in [
        MalformedHistoryField::MissingContentRef,
        MalformedHistoryField::MissingContentLocator,
        MalformedHistoryField::InvalidContentLocator,
        MalformedHistoryField::MissingContentDigest,
        MalformedHistoryField::InvalidContentDigestAlgorithm,
        MalformedHistoryField::ShortContentDigest,
        MalformedHistoryField::MissingContentSize,
        MalformedHistoryField::ZeroContentSize,
        MalformedHistoryField::OversizedContentSize,
        MalformedHistoryField::MissingSemanticFingerprint,
        MalformedHistoryField::MissingSemanticFingerprintValue,
        MalformedHistoryField::ShortSemanticFingerprint,
    ] {
        let f = Fixture::new().await?;
        let sent = f.app().send(&f.alice, f.send("malformed_content", "private")).await?;
        let id = MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
        let sender = PeerId::parse(sent.message.sender_peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
        let store = altered_history_store(&f, sender.clone(), field).await?;
        let app = PeerApplication::new(
            store,
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            f.ids.clone(),
            f.catalog.clone(),
        );
        let reads = f.store.reads.load(Ordering::SeqCst);
        let appends = f.store.appends.load(Ordering::SeqCst);
        let content_calls = f.content.calls.load(Ordering::SeqCst);
        let expected = field.expected();
        assert!(matches!(
            app.status(
                &f.alice,
                proto::StatusRequest {
                    attachment_ref: f.a.clone().into(),
                    idempotency_key: Some("malformed_content".into()),
                },
            )
            .await,
            Err(PeerApplicationError::History(HistoryError::Evolve(ref source))) if source == &expected
        ));
        assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 1);
        assert!(matches!(
            app.reconcile(&f.alice, &sender, std::slice::from_ref(&id), WorkLimit::new(1)?)
                .await,
            Err(PeerApplicationError::History(HistoryError::Evolve(ref source))) if source == &expected
        ));
        assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 2);
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
        assert!(
            f.cache
                .records
                .lock()
                .map_err(|_| PeerPortError::Unavailable)?
                .is_empty()
        );
        assert!(
            f.cache
                .orphans
                .lock()
                .map_err(|_| PeerPortError::Unavailable)?
                .is_empty()
        );
        assert_eq!(f.catalog.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn an_empty_inbox_returns_available_without_allocating_or_persisting_a_presentation() -> TestResult {
    let f = Fixture::new().await?;
    let appends = f.store.appends.load(Ordering::SeqCst);
    let identifiers = f.ids.0.load(Ordering::SeqCst);
    for _ in 0..2 {
        let response = f.app().prepare_batch(&f.bob, f.prepare()).await?;
        assert!(!response.batch.is_set());
        assert!(response.messages.is_empty());
        assert_eq!(
            response.content_availability.and_then(|value| value.as_known()),
            Some(proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE)
        );
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.ids.0.load(Ordering::SeqCst), identifiers);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn public_preparation_rejects_a_leap_clock_before_allocating_or_opening_content() -> TestResult {
    let mut f = Fixture::new().await?;
    let sent = f.intake("prepared_at_leap", "pending").await?;
    let id = MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let ordinary = f.clock.now() + Duration::seconds(58);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = ordinary;
    f.b = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .enroll(&f.bob, Fixture::enrollment("bob"))
        .await?
        .attachment
        .attachment_ref
        .clone()
        .into_option()
        .ok_or(PeerPortError::Integrity)?;
    let recipient = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let reference = AttachmentRef::new(
        recipient.clone(),
        ConnectorId::new(f.b.connector_id.clone().ok_or(PeerPortError::Integrity)?)?,
        f.b.generation.ok_or(PeerPortError::Integrity)?,
    )?;
    let batch_id = BatchId::new("leap_domain_control")?;
    let draft = proto::PeerBatch {
        batch_id: Some(batch_id.as_str().to_owned()),
        message_ids: vec![id.as_str().to_owned()],
        prepared_at: trogonai_proto::convert::timestamp_from_datetime(&ordinary).into(),
        ..Default::default()
    };
    let rendered = trogon_peer_context::PeerContext::render(
        &draft,
        &[proto::PreparedMessage {
            metadata: sent.message.clone(),
            text: Some("pending".into()),
        }],
    )?;
    let plan = TrustedBatchPlan::from_renderer(vec![id.clone()], rendered.as_str().len())?;
    let leap = chrono::Utc
        .timestamp_opt((ordinary + Duration::seconds(1)).timestamp(), 1_500_000_000)
        .single()
        .ok_or(PeerPortError::Integrity)?;
    assert_eq!(leap.timestamp_subsec_nanos(), 1_500_000_000);
    let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(f.bob.caller().as_str())?);
    let appends = f.store.appends.load(Ordering::SeqCst);
    assert!(matches!(
        CommandExecution::new(
            &f.store,
            &PrepareBatch::new(f.bob.clone(), reference, batch_id, plan, leap)
        )
        .with_principal(principal)
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await,
        Err(CommandError::Decide(MessageError::InvalidState(
            EvolveError::TimestampRange {
                field: "receipt.decided_at"
            }
        )))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = leap;
    let ids = f.ids.0.load(Ordering::SeqCst);
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    let cache_calls = f.cache.calls.load(Ordering::SeqCst);
    let app = f.app();
    let owner = f.bob.clone();
    let request = f.prepare();
    let joined = tokio::spawn(async move { app.prepare_batch(&owner, request).await }).await;
    assert!(
        joined.is_ok(),
        "public preparation must reject an unrepresentable clock without panicking"
    );
    assert!(matches!(
        joined?,
        Err(PeerApplicationError::Evidence(MessageError::InvalidState(
            EvolveError::TimestampRange {
                field: "batch.prepared_at"
            }
        )))
    ));
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), cache_calls);
    assert!(
        f.cache
            .records
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    let target = service_coverage_public_history(&f, &recipient).await?;
    assert_eq!(target.inbox.len(), 1);
    assert!(!target.inbox[0].disposition.is_set());
    assert!(target.dispositions.is_empty());
    assert!(target.batches.is_empty());
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = ordinary;
    let prepared = f.app().prepare_batch(&f.bob, f.prepare()).await?;
    assert_eq!(prepared.batch.message_ids, vec![id.as_str().to_owned()]);
    assert_eq!(
        prepared.batch.prepared_at,
        buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&ordinary))
    );
    assert_eq!(prepared.messages.len(), 1);
    assert_eq!(prepared.messages[0].metadata, sent.message);
    assert_eq!(prepared.messages[0].text.as_deref(), Some("pending"));
    assert_eq!(
        prepared.content_availability.and_then(|value| value.as_known()),
        Some(proto::ContentAvailability::CONTENT_AVAILABILITY_AVAILABLE)
    );
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids + 1);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls + 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    Ok(())
}

#[derive(Clone)]
struct StatusObservationClock {
    live_at: chrono::DateTime<chrono::Utc>,
    observed_at: chrono::DateTime<chrono::Utc>,
    reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl trogon_peer_service::PeerClock for StatusObservationClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            self.live_at
        } else {
            self.observed_at
        }
    }
}

async fn service_status_rejects_unrepresentable_observation(seconds: Option<i64>, nanos: u32) -> TestResult {
    let mut f = Fixture::new().await?;
    let sent = f
        .app()
        .send(&f.alice, f.send("status_clock", "durable message"))
        .await?;
    let renewed_at = f.clock.now() + Duration::seconds(58);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = renewed_at;
    f.a = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .enroll(&f.alice, Fixture::enrollment("alice"))
        .await?
        .attachment
        .attachment_ref
        .clone()
        .into_option()
        .ok_or(PeerPortError::Integrity)?;
    let ordinary = renewed_at + Duration::seconds(1);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = ordinary;
    let observed_at = chrono::Utc
        .timestamp_opt(seconds.unwrap_or(ordinary.timestamp()), nanos)
        .single()
        .ok_or(PeerPortError::Integrity)?;
    let clock = StatusObservationClock {
        live_at: ordinary,
        observed_at,
        reads: std::sync::Arc::default(),
    };
    let app = PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    let cache_calls = f.cache.calls.load(Ordering::SeqCst);
    let ids = f.ids.0.load(Ordering::SeqCst);
    let owner = f.alice.clone();
    let request = proto::StatusRequest {
        attachment_ref: f.a.clone().into(),
        idempotency_key: Some("status_clock".into()),
    };
    let control_request = request.clone();
    let joined = tokio::spawn(async move { app.status(&owner, request).await }).await;
    assert!(
        joined.is_ok(),
        "public status must reject an unrepresentable clock without panicking"
    );
    assert!(matches!(
        joined?,
        Err(PeerApplicationError::Evidence(MessageError::InvalidState(
            EvolveError::TimestampRange {
                field: "status.observed_at"
            }
        )))
    ));
    assert_eq!(clock.reads.load(Ordering::SeqCst), 2);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), cache_calls);
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids);
    let status = f.app().status(&f.alice, control_request).await?;
    assert_eq!(status.message, sent.message);
    assert_eq!(
        status.status.and_then(|value| value.as_known()),
        Some(proto::MessageStatus::MESSAGE_STATUS_ACCEPTED)
    );
    assert_eq!(
        status.observed_at,
        buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&ordinary))
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), cache_calls);
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids);
    Ok(())
}

#[tokio::test]
async fn public_status_rejects_a_leap_clock_after_its_live_attachment_check() -> TestResult {
    service_status_rejects_unrepresentable_observation(None, 1_500_000_000).await
}

#[tokio::test]
async fn public_status_rejects_out_of_profile_seconds_after_its_live_attachment_check() -> TestResult {
    for seconds in [-62_135_596_801, 253_402_300_800] {
        service_status_rejects_unrepresentable_observation(Some(seconds), 0).await?;
    }
    Ok(())
}

#[tokio::test]
async fn public_roster_rejects_a_leap_clock_without_emitting_invalid_presence() -> TestResult {
    let mut f = Fixture::new().await?;
    let renewed_at = f.clock.now() + Duration::seconds(58);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = renewed_at;
    f.a = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .enroll(&f.alice, Fixture::enrollment("alice"))
        .await?
        .attachment
        .attachment_ref
        .clone()
        .into_option()
        .ok_or(PeerPortError::Integrity)?;
    let ordinary = renewed_at + Duration::seconds(1);
    let leap = chrono::Utc
        .timestamp_opt(ordinary.timestamp(), 1_500_000_000)
        .single()
        .ok_or(PeerPortError::Integrity)?;
    assert_eq!(leap.timestamp_subsec_nanos(), 1_500_000_000);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = leap;
    let appends = f.store.appends.load(Ordering::SeqCst);
    let content_calls = f.content.calls.load(Ordering::SeqCst);
    let cache_calls = f.cache.calls.load(Ordering::SeqCst);
    let ids = f.ids.0.load(Ordering::SeqCst);
    let app = f.app();
    let owner = f.alice.clone();
    let request = proto::RosterRequest {
        attachment_ref: f.a.clone().into(),
        ..Default::default()
    };
    let joined = tokio::spawn(async move { app.roster(&owner, request).await }).await;
    assert!(
        joined.is_ok(),
        "public roster must reject an unrepresentable clock without panicking"
    );
    assert!(matches!(
        joined?,
        Err(PeerApplicationError::Evidence(MessageError::InvalidState(
            EvolveError::TimestampRange {
                field: "presence.observed_at"
            }
        )))
    ));
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), cache_calls);
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = ordinary;
    let roster = f
        .app()
        .roster(
            &f.alice,
            proto::RosterRequest {
                attachment_ref: f.a.clone().into(),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(roster.peers.len(), 1);
    assert_eq!(roster.peers[0].peer_id, f.a.peer_id);
    assert_eq!(
        roster.peers[0].availability.and_then(|value| value.as_known()),
        Some(proto::PeerAvailability::PEER_AVAILABILITY_READY)
    );
    for peer in roster.peers {
        assert_eq!(
            peer.observed_at,
            buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&ordinary))
        );
    }
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), content_calls);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), cache_calls);
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids);
    Ok(())
}

struct RosterObservationCatalog {
    inner: CatalogFixture,
    clock: ClockFixture,
    observed_at: chrono::DateTime<chrono::Utc>,
}

impl trogon_peer_service::PeerCatalog for RosterObservationCatalog {
    async fn page(
        &self,
        scope: &trogon_peer_service::PeerAccess,
        collaboration: &trogon_peer::CollaborationRef,
        after: Option<&trogon_peer::PeerId>,
        limit: trogon_peer_service::WorkLimit,
    ) -> Result<Vec<trogon_peer::PeerId>, trogon_peer_service::PeerPortError> {
        let members = trogon_peer_service::PeerCatalog::page(&self.inner, scope, collaboration, after, limit).await?;
        *self
            .clock
            .time
            .lock()
            .map_err(|_| trogon_peer_service::PeerPortError::Unavailable)? = self.observed_at;
        Ok(members)
    }
}

#[tokio::test]
async fn public_roster_validates_only_emitted_observations_after_a_clock_change() -> TestResult {
    for seconds in [-62_135_596_801, 253_402_300_800] {
        let f = Fixture::new().await?;
        let observed_at = chrono::Utc
            .timestamp_opt(seconds, 0)
            .single()
            .ok_or(PeerPortError::Integrity)?;
        let app = PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            f.ids.clone(),
            RosterObservationCatalog {
                inner: f.catalog.clone(),
                clock: f.clock.clone(),
                observed_at,
            },
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        let result = app
            .roster(
                &f.alice,
                proto::RosterRequest {
                    attachment_ref: f.a.clone().into(),
                    ..Default::default()
                },
            )
            .await;
        if seconds < -62_135_596_800 {
            assert!(matches!(
                result,
                Err(PeerApplicationError::Evidence(MessageError::InvalidState(
                    EvolveError::TimestampRange {
                        field: "presence.observed_at"
                    }
                )))
            ));
        } else {
            let roster = result?;
            assert!(roster.peers.is_empty());
            assert!(roster.next_page_token.is_none());
        }
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.ids.0.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn public_roster_preserves_empty_and_denied_pages_without_emitting_a_clock_observation() -> TestResult {
    for deny_member in [false, true] {
        let f = Fixture::new().await?;
        let bob = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
        *f.catalog.peers.lock().map_err(|_| PeerPortError::Unavailable)? =
            if deny_member { vec![bob.clone()] } else { vec![] };
        if deny_member {
            f.policy
                .grants
                .lock()
                .map_err(|_| PeerPortError::Unavailable)?
                .remove(bob.as_str());
        }
        let observed_at = chrono::Utc
            .timestamp_opt(253_402_300_800, 0)
            .single()
            .ok_or(PeerPortError::Integrity)?;
        let app = PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            f.clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            f.ids.clone(),
            RosterObservationCatalog {
                inner: f.catalog.clone(),
                clock: f.clock.clone(),
                observed_at,
            },
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        let roster = app
            .roster(
                &f.alice,
                proto::RosterRequest {
                    attachment_ref: f.a.clone().into(),
                    ..Default::default()
                },
            )
            .await?;
        assert!(roster.peers.is_empty());
        assert!(roster.next_page_token.is_none());
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.ids.0.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[derive(Clone)]
struct RosterAuthorityStore {
    inner: StoreFixture,
    initial_tenant: trogon_peer::TenantId,
    foreign_tenant: trogon_peer::TenantId,
    authority_changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    member: PeerId,
    member_reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl trogon_peer_service::ScopedPeerStore for RosterAuthorityStore {
    fn tenant(&self) -> &trogon_peer::TenantId {
        if self.authority_changed.load(Ordering::SeqCst) {
            &self.foreign_tenant
        } else {
            &self.initial_tenant
        }
    }
}

impl trogon_decider_runtime::StreamRead<PeerId> for RosterAuthorityStore {
    type Error = <StoreFixture as trogon_decider_runtime::StreamRead<PeerId>>::Error;

    async fn read_stream(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, PeerId>,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        if request.stream_id == &self.member {
            self.member_reads.fetch_add(1, Ordering::SeqCst);
        }
        trogon_decider_runtime::StreamRead::read_stream(&self.inner, request).await
    }

    async fn read_stream_bounded(
        &self,
        request: trogon_decider_runtime::ReadStreamRequest<'_, PeerId>,
        maximum: u64,
    ) -> Result<trogon_decider_runtime::ReadStreamResponse, Self::Error> {
        if request.stream_id == &self.member {
            self.member_reads.fetch_add(1, Ordering::SeqCst);
        }
        trogon_decider_runtime::StreamRead::read_stream_bounded(&self.inner, request, maximum).await
    }
}

impl trogon_decider_runtime::StreamAppend<PeerId> for RosterAuthorityStore {
    type Error = <StoreFixture as trogon_decider_runtime::StreamAppend<PeerId>>::Error;

    async fn append_stream(
        &self,
        request: trogon_decider_runtime::AppendStreamRequest<'_, PeerId>,
    ) -> Result<trogon_decider_runtime::AppendStreamResponse, Self::Error> {
        trogon_decider_runtime::StreamAppend::append_stream(&self.inner, request).await
    }

    fn classify_append_failure(&self, error: &Self::Error) -> trogon_decider_runtime::AppendFailure {
        trogon_decider_runtime::StreamAppend::classify_append_failure(&self.inner, error)
    }
}

struct DeniedRosterMemberPolicy {
    inner: PolicyFixture,
    member: PeerId,
}

impl trogon_decider_runtime::CommandAuthorizer<trogon_peer_service::PeerAccess> for DeniedRosterMemberPolicy {
    fn authorize(
        &self,
        principal: &trogon_decider_runtime::CommandPrincipal,
        scope: &trogon_peer_service::PeerAccess,
    ) -> Result<(), trogon_decider_runtime::AuthorizationDeniedError> {
        <PolicyFixture as trogon_decider_runtime::CommandAuthorizer<trogon_peer_service::PeerAccess>>::authorize(
            &self.inner,
            principal,
            scope,
        )?;
        if scope.operation() == trogon_peer_service::PeerOperation::ReadPeer && scope.peer() == &self.member {
            return Err(trogon_decider_runtime::AuthorizationDeniedError::new(
                "roster member denied",
            ));
        }
        Ok(())
    }
}

struct AuthorityChangingRosterCatalog {
    inner: CatalogFixture,
    authority_changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl trogon_peer_service::PeerCatalog for AuthorityChangingRosterCatalog {
    async fn page(
        &self,
        scope: &trogon_peer_service::PeerAccess,
        collaboration: &trogon_peer::CollaborationRef,
        after: Option<&PeerId>,
        limit: WorkLimit,
    ) -> Result<Vec<PeerId>, trogon_peer_service::PeerPortError> {
        let members = trogon_peer_service::PeerCatalog::page(&self.inner, scope, collaboration, after, limit).await?;
        self.authority_changed.store(true, Ordering::SeqCst);
        Ok(members)
    }
}

#[tokio::test]
async fn a_denied_roster_member_is_omitted_without_loading_its_history() -> TestResult {
    let f = Fixture::new().await?;
    let requester = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let member = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    assert_ne!(requester, member);
    let store = RosterAuthorityStore {
        inner: f.store.clone(),
        initial_tenant: f.store.tenant.clone(),
        foreign_tenant: trogon_peer::TenantId::new("foreign")?,
        authority_changed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        member: member.clone(),
        member_reads: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let request = proto::RosterRequest {
        attachment_ref: f.a.clone().into(),
        page_size: Some(3),
        page_token: None,
    };
    let app = PeerApplication::new(
        store.clone(),
        DeniedRosterMemberPolicy {
            inner: f.policy.clone(),
            member: member.clone(),
        },
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let reads = f.store.reads.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    let catalog_calls = f.catalog.calls.load(Ordering::SeqCst);
    let roster = app.roster(&f.alice, request.clone()).await?;
    assert_eq!(roster.peers.len(), 1);
    assert_eq!(roster.peers[0].peer_id.as_deref(), Some(requester.as_str()));
    assert!(roster.next_page_token.is_none());
    assert_eq!(store.member_reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 2);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), catalog_calls + 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    let ordinary = PeerApplication::new(
        store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    )
    .roster(&f.alice, request)
    .await?;
    assert_eq!(ordinary.peers.len(), 2);
    assert!(
        ordinary
            .peers
            .iter()
            .any(|peer| peer.peer_id.as_deref() == Some(requester.as_str()))
    );
    assert!(
        ordinary
            .peers
            .iter()
            .any(|peer| peer.peer_id.as_deref() == Some(member.as_str()))
    );
    assert_eq!(store.member_reads.load(Ordering::SeqCst), 1);
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 5);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}

#[tokio::test]
async fn roster_rechecks_store_tenant_after_catalog_await_before_loading_a_member() -> TestResult {
    let f = Fixture::new().await?;
    let member = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let authority_changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store = RosterAuthorityStore {
        inner: f.store.clone(),
        initial_tenant: f.store.tenant.clone(),
        foreign_tenant: trogon_peer::TenantId::new("foreign")?,
        authority_changed: authority_changed.clone(),
        member: member.clone(),
        member_reads: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    assert_eq!(store.tenant(), f.alice.tenant());
    let request = proto::RosterRequest {
        attachment_ref: f.a.clone().into(),
        page_size: Some(3),
        page_token: None,
    };
    let app = PeerApplication::new(
        store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        AuthorityChangingRosterCatalog {
            inner: f.catalog.clone(),
            authority_changed: authority_changed.clone(),
        },
    );
    let reads = f.store.reads.load(Ordering::SeqCst);
    let appends = f.store.appends.load(Ordering::SeqCst);
    let catalog_calls = f.catalog.calls.load(Ordering::SeqCst);
    assert!(matches!(
        app.roster(&f.alice, request.clone()).await,
        Err(PeerApplicationError::Scope)
    ));
    assert!(authority_changed.load(Ordering::SeqCst));
    assert_eq!(store.tenant(), &store.foreign_tenant);
    assert_eq!(f.catalog.calls.load(Ordering::SeqCst), catalog_calls + 1);
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 1);
    assert_eq!(store.member_reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert!(
        f.cache
            .records
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    assert!(
        f.cache
            .orphans
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .is_empty()
    );
    authority_changed.store(false, Ordering::SeqCst);
    assert_eq!(store.tenant(), f.alice.tenant());
    let ordinary = PeerApplication::new(
        store.clone(),
        f.policy.clone(),
        f.clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    )
    .roster(&f.alice, request)
    .await?;
    assert_eq!(ordinary.peers.len(), 2);
    assert!(
        ordinary
            .peers
            .iter()
            .any(|peer| peer.peer_id.as_deref() == Some(member.as_str()))
    );
    assert_eq!(f.store.reads.load(Ordering::SeqCst), reads + 4);
    assert_eq!(store.member_reads.load(Ordering::SeqCst), 1);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    Ok(())
}

#[derive(Clone)]
struct SendAdmissionObservationClock {
    live_at: chrono::DateTime<chrono::Utc>,
    admission_at: chrono::DateTime<chrono::Utc>,
    reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl trogon_peer_service::PeerClock for SendAdmissionObservationClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 1 {
            self.admission_at
        } else {
            self.live_at
        }
    }
}

#[tokio::test]
async fn public_send_rejects_a_leap_admission_clock_before_sealing_and_keeps_exact_retry() -> TestResult {
    let mut f = Fixture::new().await?;
    let renewed_at = f.clock.now() + Duration::seconds(58);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = renewed_at;
    f.a = PeerService::new(f.store.clone(), f.policy.clone(), f.clock.clone())
        .enroll(&f.alice, Fixture::enrollment("alice"))
        .await?
        .attachment
        .attachment_ref
        .clone()
        .into_option()
        .ok_or(PeerPortError::Integrity)?;
    let ordinary = renewed_at + Duration::seconds(1);
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = ordinary;
    let leap = chrono::Utc
        .timestamp_opt(ordinary.timestamp(), 1_500_000_000)
        .single()
        .ok_or(PeerPortError::Integrity)?;
    let clock = SendAdmissionObservationClock {
        live_at: ordinary,
        admission_at: leap,
        reads: std::sync::Arc::default(),
    };
    let app = PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let appends = f.store.appends.load(Ordering::SeqCst);
    let ids = f.ids.0.load(Ordering::SeqCst);
    let owner = f.alice.clone();
    let request = f.send("admission_clock", "durable message");
    let joined = tokio::spawn(async move { app.send(&owner, request).await }).await;
    assert!(
        joined.is_ok(),
        "public send must reject an unrepresentable admission clock without panicking"
    );
    assert!(matches!(
        joined?,
        Err(PeerApplicationError::Evidence(MessageError::InvalidState(
            EvolveError::TimestampRange { field: "accepted_at" }
        )))
    ));
    assert_eq!(clock.reads.load(Ordering::SeqCst), 2);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids + 1);
    let accepted = f
        .app()
        .send(&f.alice, f.send("admission_clock", "durable message"))
        .await?;
    assert_eq!(
        accepted.message.accepted_at,
        buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&ordinary))
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    let retry_clock = SendAdmissionObservationClock {
        live_at: ordinary,
        admission_at: leap,
        reads: std::sync::Arc::default(),
    };
    let retry_app = PeerApplication::new(
        f.store.clone(),
        f.policy.clone(),
        retry_clock.clone(),
        f.content.clone(),
        f.cache.clone(),
        f.ids.clone(),
        f.catalog.clone(),
    );
    let retry = retry_app
        .send(&f.alice, f.send("admission_clock", "durable message"))
        .await?;
    assert_eq!(retry, accepted);
    assert_eq!(retry_clock.reads.load(Ordering::SeqCst), 3);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), appends + 1);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.ids.0.load(Ordering::SeqCst), ids + 3);
    Ok(())
}

#[tokio::test]
async fn public_send_keeps_unsupported_admission_seconds_owned_by_the_renderer() -> TestResult {
    for seconds in [-62_135_596_801, 253_402_300_800] {
        let f = Fixture::new().await?;
        let invalid = chrono::Utc
            .timestamp_opt(seconds, 0)
            .single()
            .ok_or(PeerPortError::Integrity)?;
        let clock = SendAdmissionObservationClock {
            live_at: f.clock.now(),
            admission_at: invalid,
            reads: std::sync::Arc::default(),
        };
        let app = PeerApplication::new(
            f.store.clone(),
            f.policy.clone(),
            clock.clone(),
            f.content.clone(),
            f.cache.clone(),
            f.ids.clone(),
            f.catalog.clone(),
        );
        let appends = f.store.appends.load(Ordering::SeqCst);
        assert!(matches!(
            app.send(&f.alice, f.send("admission_seconds", "durable message")).await,
            Err(PeerApplicationError::Context(
                trogon_peer_context::PeerContextError::ContractViolation
            ))
        ));
        assert_eq!(clock.reads.load(Ordering::SeqCst), 2);
        assert_eq!(f.store.appends.load(Ordering::SeqCst), appends);
        assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
        assert_eq!(f.ids.0.load(Ordering::SeqCst), 1);
    }
    Ok(())
}
