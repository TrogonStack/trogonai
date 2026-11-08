use std::sync::atomic::Ordering;
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
async fn unimplemented_operations_return_typed_errors_without_calling_ports() -> TestResult {
    use trogon_peer_service::PeerApplicationError;
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
        Err(PeerApplicationError::NotImplemented)
    ));
    assert!(matches!(
        app.prepare_batch(&f.bob, Default::default()).await,
        Err(PeerApplicationError::NotImplemented)
    ));
    assert!(matches!(
        app.acknowledge_batch(&f.bob, Default::default()).await,
        Err(PeerApplicationError::NotImplemented)
    ));
    assert!(matches!(
        app.roster(&f.bob, Default::default()).await,
        Err(PeerApplicationError::NotImplemented)
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
    use trogon_decider_runtime::{CommandExecution, CommandPrincipal, PrincipalId, PrincipalKind};
    use trogon_peer::{
        AcceptedMessage, AcknowledgeBatch, AttachmentRef, BatchId, ConnectorId, IntakeMessage, MessageId, PeerId,
        PrepareBatch, TrustedBatchPlan,
    };
    use trogon_peer_service::{
        PeerClock, PeerPortError, PreparedPresentation, PresentationInsert, PresentationKey, PresentationRead,
    };
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

#[derive(Clone, Copy)]
enum MalformedHistoryField {
    MissingCollaboration,
    EmptyCollaboration,
    ControlCollaboration,
    OversizedCollaboration,
    MissingRecipient,
    InvalidRecipient,
}

impl MalformedHistoryField {
    fn expected(self) -> trogon_peer::EvolveError {
        use trogon_peer::{EvolveError, PeerValueError};
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
    use trogon_decider_runtime::{EventEncode, ReadFrom, ReadStreamRequest, StreamRead};
    use trogonai_proto::peers::PeerEventCase;
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
    use chrono::{Duration, TimeZone};
    use trogon_peer::{EvolveError, MessageError};
    use trogon_peer_service::{PeerApplication, PeerApplicationError, PeerClock, PeerPortError, PeerService};
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
