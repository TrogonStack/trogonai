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
    use trogon_decider_runtime::{CommandAuthorizer as _, CommandPrincipal, PrincipalId, PrincipalKind};
    use trogon_peer::{PeerId, SealedContentRef};
    use trogon_peer_service::{
        ContentBinding, ContentOpen as _, ContentSeal as _, MessageText, PeerAccess, PeerOperation, PeerPortError,
    };

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
    use trogon_peer_service::{MessageText, PeerApplicationError};
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
    use trogon_decider_runtime::StreamRead;
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
    use trogon_decider_runtime::{StreamAppend, StreamRead};
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
