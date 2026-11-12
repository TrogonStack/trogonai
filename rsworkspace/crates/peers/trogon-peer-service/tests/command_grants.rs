use std::sync::atomic::Ordering;
use trogon_peer::Attach;
use trogon_peer::Enroll;
use trogon_peer::EnrollmentIdentity;
use trogon_peer::Heartbeat;
use trogon_peer::InstallationId;
use trogon_peer::NativeSessionId;
use trogon_peer::PeerDisplayName;
use trogon_peer::PeerObservation;
use trogon_peer_service::PeerService;
use trogon_peer_service::test_support::IsolatedEnrollmentFixture;

mod support;
use support::*;
use trogon_decider_runtime::{
    AuthorizationDeniedError, CommandAuthorizer, CommandError, CommandExecution, CommandPrincipal, PrincipalId,
    PrincipalKind, ReadFrom, ReadStreamRequest, ReplayLimit, StreamRead, UnauthorizedError,
};
use trogon_peer::{
    AcceptedMessage, AcknowledgeBatch, AttachmentRef, AuthoritativeReceipt, BatchId, CollaborationRef, ConnectorId,
    DisposeMessage, IdempotencyKey, IntakeMessage, MessageId, PeerId, PrepareBatch, SendMessage, SettleMessage,
    TerminalDisposition, TrustedBatchPlan, TrustedContent, TrustedPeerOwner,
};
use trogon_peer_service::{ContentSeal, MessageText, PeerAccess, PeerClock, PeerOperation, PeerPortError};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};

fn principal(owner: &TrustedPeerOwner) -> TestResult<CommandPrincipal> {
    Ok(CommandPrincipal::new(
        PrincipalKind::Agent,
        PrincipalId::new(owner.caller().as_str())?,
    ))
}

fn reference(value: &proto::AttachmentRef) -> TestResult<AttachmentRef> {
    Ok(AttachmentRef::new(
        PeerId::parse(value.peer_id.clone().ok_or(PeerPortError::Integrity)?)?,
        ConnectorId::new(value.connector_id.clone().ok_or(PeerPortError::Integrity)?)?,
        value.generation.ok_or(PeerPortError::Integrity)?,
    )?)
}

fn storage_calls(f: &Fixture) -> (usize, usize) {
    (
        f.store.reads.load(Ordering::SeqCst),
        f.store.appends.load(Ordering::SeqCst),
    )
}

macro_rules! denied_before_storage {
    ($fixture:expr, $command:expr, $principal:expr, $grant:expr) => {{
        let before = storage_calls($fixture);
        let result = CommandExecution::new(&$fixture.store, $command)
            .with_principal($principal.clone())
            .with_authorizer($grant)
            .with_replay_limit(ReplayLimit::try_new(4096)?)
            .execute()
            .await;
        assert!(matches!(
            result,
            Err(CommandError::Unauthorized(UnauthorizedError::Denied(_)))
        ));
        assert_eq!(storage_calls($fixture), before, "denied grant reached storage");
    }};
}

async fn history(f: &Fixture, peer: &PeerId) -> TestResult<PeerState> {
    let read = f
        .store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: peer,
                from: ReadFrom::Beginning,
            },
            4096,
        )
        .await?;
    let mut state = trogon_peer::commands::state::initial_state();
    for event in read.events {
        let event = event
            .decode::<proto::PeerEvent>()?
            .into_decoded()
            .ok_or(PeerPortError::Integrity)?;
        state = trogon_peer::commands::state::evolve(state, &event)?;
    }
    Ok(state)
}

struct AcceptedFixture {
    sender: PeerId,
    recipient: PeerId,
    message: MessageId,
    evidence: AcceptedMessage,
}

async fn accepted(f: &Fixture, key: &str) -> TestResult<AcceptedFixture> {
    let sent = f.app().send(&f.alice, f.send(key, "payload")).await?;
    let message = MessageId::new(sent.message.message_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let sender = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let recipient = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let evidence = AcceptedMessage::from_sender_history(&history(f, &sender).await?, &message)?;
    Ok(AcceptedFixture {
        sender,
        recipient,
        message,
        evidence,
    })
}

#[derive(Clone)]
struct SendGrant {
    base: PolicyFixture,
    recipient: PeerId,
    collaboration: CollaborationRef,
    content: TrustedContent,
}

impl CommandAuthorizer<SendMessage> for SendGrant {
    fn authorize(&self, principal: &CommandPrincipal, command: &SendMessage) -> Result<(), AuthorizationDeniedError> {
        self.base.authorize(principal, command)?;
        if command.recipient() != &self.recipient
            || command.collaboration() != &self.collaboration
            || command.content() != &self.content
        {
            return Err(AuthorizationDeniedError::new("send exceeds sealed-content grant"));
        }
        Ok(())
    }
}

#[derive(Clone)]
struct PreparationGrant {
    base: PolicyFixture,
    batch: BatchId,
    messages: Vec<MessageId>,
    maximum_rendered_bytes: usize,
}

impl CommandAuthorizer<PrepareBatch> for PreparationGrant {
    fn authorize(&self, principal: &CommandPrincipal, command: &PrepareBatch) -> Result<(), AuthorizationDeniedError> {
        self.base.authorize(principal, command)?;
        if command.batch_id() != &self.batch
            || command.plan().message_ids() != self.messages.as_slice()
            || command.plan().rendered_utf8_bytes() > self.maximum_rendered_bytes
        {
            return Err(AuthorizationDeniedError::new("preparation exceeds rendered-plan grant"));
        }
        Ok(())
    }
}

struct AcknowledgementGrant {
    base: PolicyFixture,
    batch: BatchId,
}

impl CommandAuthorizer<AcknowledgeBatch> for AcknowledgementGrant {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        command: &AcknowledgeBatch,
    ) -> Result<(), AuthorizationDeniedError> {
        self.base.authorize(principal, command)?;
        if command.batch_id() != &self.batch {
            return Err(AuthorizationDeniedError::new("batch acknowledgement not granted"));
        }
        Ok(())
    }
}

#[derive(Clone)]
struct DispositionGrant {
    base: PolicyFixture,
    actor: TrustedPeerOwner,
    message: MessageId,
    outcome: TerminalDisposition,
}

impl CommandAuthorizer<DisposeMessage> for DispositionGrant {
    fn authorize(
        &self,
        principal: &CommandPrincipal,
        command: &DisposeMessage,
    ) -> Result<(), AuthorizationDeniedError> {
        self.base.owner(principal, &self.actor)?;
        self.base.authorize(principal, command)?;
        if command.evidence().metadata().message_id.as_deref() != Some(self.message.as_str())
            || command.disposition() != self.outcome
        {
            return Err(AuthorizationDeniedError::new("terminal outcome not granted"));
        }
        Ok(())
    }
}

#[derive(Clone)]
struct SettlementGrant {
    base: PolicyFixture,
    actor: TrustedPeerOwner,
    sender: PeerId,
    recipient: PeerId,
    message: MessageId,
    outcome: proto::PreparationOutcome,
}

impl CommandAuthorizer<SettleMessage> for SettlementGrant {
    fn authorize(&self, principal: &CommandPrincipal, command: &SettleMessage) -> Result<(), AuthorizationDeniedError> {
        self.base.owner(principal, &self.actor)?;
        self.base.authorize(principal, command)?;
        let receipt = command.evidence().receipt();
        if command.sender() != &self.sender
            || receipt.recipient_peer_id.as_deref() != Some(self.recipient.as_str())
            || receipt.message_id.as_deref() != Some(self.message.as_str())
            || receipt.outcome.and_then(|outcome| outcome.as_known()) != Some(self.outcome)
        {
            return Err(AuthorizationDeniedError::new("authoritative settlement not granted"));
        }
        Ok(())
    }
}

#[tokio::test]
async fn sealed_send_grant_binds_recipient_collaboration_and_content_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let sender = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let recipient = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let scope = PeerAccess::new(f.alice.clone(), sender.clone(), PeerOperation::Send);
    let sealed = f
        .content
        .seal(&scope, &MessageText::new("permitted body".into())?)
        .await?;
    let other = f
        .content
        .seal(&scope, &MessageText::new("different body".into())?)
        .await?;
    let command = SendMessage::new(
        f.alice.clone(),
        reference(&f.a)?,
        IdempotencyKey::new("granted_send")?,
        MessageId::new("granted_message")?,
        recipient.clone(),
        CollaborationRef::new("shared")?,
        sealed.content().clone(),
        None,
        f.clock.now(),
    );
    let allowed = SendGrant {
        base: f.policy.clone(),
        recipient,
        collaboration: CollaborationRef::new("shared")?,
        content: sealed.content().clone(),
    };
    let principal = principal(&f.alice)?;
    for denied in [
        SendGrant {
            recipient: sender.clone(),
            ..allowed.clone()
        },
        SendGrant {
            collaboration: CollaborationRef::new("ungranted")?,
            ..allowed.clone()
        },
        SendGrant {
            content: other.content().clone(),
            ..allowed.clone()
        },
    ] {
        denied_before_storage!(&f, &command, principal, &denied);
    }
    CommandExecution::new(&f.store, &command)
        .with_principal(principal)
        .with_authorizer(&allowed)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let persisted = history(&f, &sender).await?;
    assert_eq!(persisted.outbox.len(), 1);
    assert_eq!(
        persisted.outbox[0].message.message_id.as_deref(),
        Some("granted_message")
    );
    assert_eq!(
        persisted.outbox[0].message.content_ref.as_option(),
        Some(&sealed.content().reference().to_claim_check())
    );
    Ok(())
}

#[tokio::test]
async fn batch_grants_bind_selection_budget_and_exact_acknowledgement_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let accepted = accepted(&f, "batch_grant").await?;
    let principal = principal(&f.bob)?;
    CommandExecution::new(&f.store, &IntakeMessage::new(accepted.evidence.clone(), f.clock.now()))
        .with_principal(principal.clone())
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let batch = BatchId::new("granted_batch")?;
    let command = PrepareBatch::new(
        f.bob.clone(),
        reference(&f.b)?,
        batch.clone(),
        TrustedBatchPlan::from_renderer(vec![accepted.message.clone()], 20)?,
        f.clock.now(),
    );
    let allowed = PreparationGrant {
        base: f.policy.clone(),
        batch: batch.clone(),
        messages: vec![accepted.message.clone()],
        maximum_rendered_bytes: 20,
    };
    for denied in [
        PreparationGrant {
            batch: BatchId::new("different_batch")?,
            ..allowed.clone()
        },
        PreparationGrant {
            messages: vec![MessageId::new("different_message")?],
            ..allowed.clone()
        },
        PreparationGrant {
            maximum_rendered_bytes: 19,
            ..allowed.clone()
        },
    ] {
        denied_before_storage!(&f, &command, principal, &denied);
    }
    CommandExecution::new(&f.store, &command)
        .with_principal(principal.clone())
        .with_authorizer(&allowed)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let prepared = history(&f, &accepted.recipient).await?;
    assert_eq!(prepared.batches.len(), 1);
    assert_eq!(
        prepared.batches[0].message_ids,
        vec![accepted.message.as_str().to_owned()]
    );
    assert!(!prepared.batches[0].acknowledged_at.is_set());
    let command = AcknowledgeBatch::new(f.bob.clone(), reference(&f.b)?, batch.clone(), f.clock.now());
    let denied = AcknowledgementGrant {
        base: f.policy.clone(),
        batch: BatchId::new("different_batch")?,
    };
    denied_before_storage!(&f, &command, principal, &denied);
    let allowed = AcknowledgementGrant {
        base: f.policy.clone(),
        batch,
    };
    CommandExecution::new(&f.store, &command)
        .with_principal(principal)
        .with_authorizer(&allowed)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let acknowledged = history(&f, &accepted.recipient).await?;
    assert!(acknowledged.batches[0].acknowledged_at.is_set());
    assert_eq!(acknowledged.batches[0].message_ids, prepared.batches[0].message_ids);
    Ok(())
}

#[tokio::test]
async fn terminal_and_settlement_grants_bind_authoritative_outcomes_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let accepted = accepted(&f, "terminal_grant").await?;
    let recipient_principal = principal(&f.bob)?;
    let command = DisposeMessage::new(accepted.evidence, TerminalDisposition::Unavailable, f.clock.now());
    let allowed = DispositionGrant {
        base: f.policy.clone(),
        actor: f.bob.clone(),
        message: accepted.message.clone(),
        outcome: TerminalDisposition::Unavailable,
    };
    for denied in [
        DispositionGrant {
            message: MessageId::new("different_message")?,
            ..allowed.clone()
        },
        DispositionGrant {
            outcome: TerminalDisposition::Expired,
            ..allowed.clone()
        },
    ] {
        denied_before_storage!(&f, &command, recipient_principal, &denied);
    }
    CommandExecution::new(&f.store, &command)
        .with_principal(recipient_principal)
        .with_authorizer(&allowed)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let recipient = history(&f, &accepted.recipient).await?;
    assert!(recipient.inbox.is_empty());
    let proof = AuthoritativeReceipt::from_recipient_history(&recipient, &accepted.message)?;
    let command = SettleMessage::new(accepted.sender.clone(), proof);
    let principal = principal(&f.alice)?;
    let allowed = SettlementGrant {
        base: f.policy.clone(),
        actor: f.alice.clone(),
        sender: accepted.sender.clone(),
        recipient: accepted.recipient.clone(),
        message: accepted.message.clone(),
        outcome: proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE,
    };
    for denied in [
        SettlementGrant {
            sender: accepted.recipient.clone(),
            ..allowed.clone()
        },
        SettlementGrant {
            recipient: accepted.sender.clone(),
            ..allowed.clone()
        },
        SettlementGrant {
            message: MessageId::new("different_message")?,
            ..allowed.clone()
        },
        SettlementGrant {
            outcome: proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED,
            ..allowed.clone()
        },
    ] {
        denied_before_storage!(&f, &command, principal, &denied);
    }
    CommandExecution::new(&f.store, &command)
        .with_principal(principal)
        .with_authorizer(&allowed)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let sender = history(&f, &accepted.sender).await?;
    assert_eq!(sender.outbox.len(), 1);
    assert_eq!(sender.outbox[0].settlement.as_option(), recipient.dispositions.first());
    assert_eq!(
        sender.outbox[0]
            .settlement
            .outcome
            .and_then(|outcome| outcome.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE)
    );
    Ok(())
}

#[tokio::test]
async fn public_fixture_shared_enrollment_and_resume_reject_other_collaborations_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let identity = EnrollmentIdentity::new(
        f.alice.clone(),
        InstallationId::new("install")?,
        NativeSessionId::new("shared_policy_control")?,
    );
    let actor = principal(&f.alice)?;
    let denied = Enroll::new(
        identity.clone(),
        ConnectorId::new("original")?,
        CollaborationRef::new("other")?,
        PeerDisplayName::new("Shared policy control")?,
        f.clock.now(),
    );
    denied_before_storage!(&f, &denied, actor, &f.policy);
    let allowed = Enroll::new(
        identity.clone(),
        ConnectorId::new("original")?,
        CollaborationRef::new("shared")?,
        PeerDisplayName::new("Shared policy control")?,
        f.clock.now(),
    );
    let enrolled = CommandExecution::new(&f.store, &allowed)
        .with_principal(actor.clone())
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?
        .state;
    assert_eq!(
        enrolled.enrollment.peer_id.as_deref(),
        Some(identity.peer_id().as_str())
    );
    assert_eq!(enrolled.enrollment.collaboration_ref.as_deref(), Some("shared"));
    assert_eq!(enrolled.attachment.attachment_ref.generation, Some(1));
    let expires = trogonai_proto::convert::datetime_from_timestamp(
        enrolled
            .attachment
            .expires_at
            .as_option()
            .ok_or(PeerPortError::Integrity)?,
    )?;
    let denied = Attach::new(
        identity.clone(),
        ConnectorId::new("replacement")?,
        CollaborationRef::new("other")?,
        expires,
    );
    denied_before_storage!(&f, &denied, actor, &f.policy);
    let allowed = Attach::new(
        identity.clone(),
        ConnectorId::new("replacement")?,
        CollaborationRef::new("shared")?,
        expires,
    );
    let before = storage_calls(&f);
    let resumed = CommandExecution::new(&f.store, &allowed)
        .with_principal(actor)
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?
        .state;
    assert_eq!(resumed.enrollment, enrolled.enrollment);
    assert_eq!(resumed.attachment.attachment_ref.generation, Some(2));
    assert_eq!(
        resumed.attachment.attachment_ref.connector_id.as_deref(),
        Some("replacement")
    );
    assert_eq!(resumed.attachment.observed_at, enrolled.attachment.expires_at);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before.1 + 1);
    assert_eq!(history(&f, identity.peer_id()).await?, resumed);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn public_isolated_fixture_authorizes_real_expired_resume_and_denies_other_collaborations_before_storage()
-> TestResult {
    let f = Fixture::new().await?;
    let identity = EnrollmentIdentity::new(
        f.alice.clone(),
        InstallationId::new("install")?,
        NativeSessionId::new("isolated_policy_control")?,
    );
    let actor = principal(&f.alice)?;
    let isolated = IsolatedEnrollmentFixture(f.alice.clone());
    let denied = Enroll::new(
        identity.clone(),
        ConnectorId::new("original")?,
        CollaborationRef::new("shared")?,
        PeerDisplayName::new("Isolated policy control")?,
        f.clock.now(),
    );
    denied_before_storage!(&f, &denied, actor, &isolated);
    let service = PeerService::new(
        f.store.clone(),
        IsolatedEnrollmentFixture(f.alice.clone()),
        f.clock.clone(),
    );
    let mut request = Fixture::enrollment("isolated_policy_control");
    request.collaboration_ref = Some("isolated".into());
    request.connector_id = Some("original".into());
    let first = service.enroll(&f.alice, request.clone()).await?;
    assert_eq!(
        first.attachment.attachment_ref.peer_id.as_deref(),
        Some(identity.peer_id().as_str())
    );
    assert_eq!(first.attachment.attachment_ref.generation, Some(1));
    let enrolled = history(&f, identity.peer_id()).await?;
    assert_eq!(enrolled.enrollment.collaboration_ref.as_deref(), Some("isolated"));
    let expires = trogonai_proto::convert::datetime_from_timestamp(
        first
            .attachment
            .expires_at
            .as_option()
            .ok_or(PeerPortError::Integrity)?,
    )?;
    *f.clock.time.lock().map_err(|_| PeerPortError::Unavailable)? = expires;
    let denied = Attach::new(
        identity.clone(),
        ConnectorId::new("replacement")?,
        CollaborationRef::new("shared")?,
        expires,
    );
    denied_before_storage!(&f, &denied, actor, &isolated);
    let events_before = f
        .store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: identity.peer_id(),
                from: ReadFrom::Beginning,
            },
            4096,
        )
        .await?
        .events
        .len();
    request.connector_id = Some("replacement".into());
    let before = storage_calls(&f);
    let resumed = service.enroll(&f.alice, request).await?;
    assert_eq!(
        resumed.attachment.attachment_ref.peer_id.as_deref(),
        Some(identity.peer_id().as_str())
    );
    assert_eq!(resumed.attachment.attachment_ref.generation, Some(2));
    assert_eq!(
        resumed.attachment.attachment_ref.connector_id.as_deref(),
        Some("replacement")
    );
    assert_eq!(resumed.attachment.observed_at, first.attachment.expires_at);
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before.1 + 2);
    let events_after = f
        .store
        .read_stream_bounded(
            ReadStreamRequest {
                stream_id: identity.peer_id(),
                from: ReadFrom::Beginning,
            },
            4096,
        )
        .await?
        .events
        .len();
    assert_eq!(events_after, events_before + 1);
    let persisted = history(&f, identity.peer_id()).await?;
    assert_eq!(persisted.enrollment, enrolled.enrollment);
    assert_eq!(persisted.attachment, resumed.attachment);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn public_fixture_owner_grant_rejects_another_peers_heartbeat_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let actor = principal(&f.alice)?;
    let observation = PeerObservation::new(proto::PeerAvailability::PEER_AVAILABILITY_READY, None, None)?;
    let denied = Heartbeat::new(f.alice.clone(), reference(&f.b)?, observation.clone(), f.clock.now());
    denied_before_storage!(&f, &denied, actor, &f.policy);
    let command = Heartbeat::new(f.alice.clone(), reference(&f.a)?, observation, f.clock.now());
    let before = storage_calls(&f);
    let updated = CommandExecution::new(&f.store, &command)
        .with_principal(actor)
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?
        .state;
    assert_eq!(
        updated.availability.and_then(|value| value.as_known()),
        Some(proto::PeerAvailability::PEER_AVAILABILITY_READY)
    );
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before.1 + 1);
    assert_eq!(history(&f, command.attachment().peer_id()).await?, updated);
    assert_eq!(f.content.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn public_fixture_revoked_peer_grants_block_evidence_and_settlement_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let accepted = accepted(&f, "base_evidence_grants").await?;
    let recipient_actor = principal(&f.bob)?;
    let intake = IntakeMessage::new(accepted.evidence.clone(), f.clock.now());
    let disposition = DisposeMessage::new(
        accepted.evidence.clone(),
        TerminalDisposition::Unavailable,
        f.clock.now(),
    );
    for peer in [&accepted.sender, &accepted.recipient] {
        let owner = f
            .policy
            .grants
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .remove(peer.as_str())
            .ok_or(PeerPortError::Integrity)?;
        denied_before_storage!(&f, &intake, recipient_actor, &f.policy);
        denied_before_storage!(&f, &disposition, recipient_actor, &f.policy);
        assert!(
            f.policy
                .grants
                .lock()
                .map_err(|_| PeerPortError::Unavailable)?
                .insert(peer.as_str().to_owned(), owner)
                .is_none()
        );
    }
    CommandExecution::new(&f.store, &intake)
        .with_principal(recipient_actor.clone())
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let intaken = history(&f, &accepted.recipient).await?;
    assert_eq!(intaken.inbox.len(), 1);
    assert_eq!(intaken.inbox[0].message.as_option(), Some(accepted.evidence.metadata()));
    assert!(intaken.dispositions.is_empty());
    CommandExecution::new(&f.store, &disposition)
        .with_principal(recipient_actor)
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let disposed = history(&f, &accepted.recipient).await?;
    assert_eq!(disposed.dispositions.len(), 1);
    assert_eq!(disposed.inbox[0].disposition.as_option(), disposed.dispositions.first());
    assert_eq!(
        disposed.dispositions[0].outcome.and_then(|value| value.as_known()),
        Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE)
    );
    let receipt = AuthoritativeReceipt::from_recipient_history(&disposed, &accepted.message)?;
    let settlement = SettleMessage::new(accepted.sender.clone(), receipt);
    let sender_actor = principal(&f.alice)?;
    let owner = f
        .policy
        .grants
        .lock()
        .map_err(|_| PeerPortError::Unavailable)?
        .remove(accepted.sender.as_str())
        .ok_or(PeerPortError::Integrity)?;
    denied_before_storage!(&f, &settlement, sender_actor, &f.policy);
    assert!(
        f.policy
            .grants
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .insert(accepted.sender.as_str().to_owned(), owner)
            .is_none()
    );
    let before = storage_calls(&f);
    CommandExecution::new(&f.store, &settlement)
        .with_principal(sender_actor)
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before.1 + 1);
    let settled = history(&f, &accepted.sender).await?;
    assert_eq!(settled.outbox.len(), 1);
    assert_eq!(settled.outbox[0].settlement.as_option(), disposed.dispositions.first());
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn public_fixture_send_metadata_grant_rejects_collaboration_and_revoked_recipient_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let sender = PeerId::parse(f.a.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let recipient = PeerId::parse(f.b.peer_id.clone().ok_or(PeerPortError::Integrity)?)?;
    let scope = PeerAccess::new(f.alice.clone(), sender.clone(), PeerOperation::Send);
    let sealed = f
        .content
        .seal(&scope, &MessageText::new("real fixture body".into())?)
        .await?;
    let actor = principal(&f.alice)?;
    let denied = SendMessage::new(
        f.alice.clone(),
        reference(&f.a)?,
        IdempotencyKey::new("base_metadata_grant")?,
        MessageId::new("base_metadata_message")?,
        recipient.clone(),
        CollaborationRef::new("other")?,
        sealed.content().clone(),
        None,
        f.clock.now(),
    );
    denied_before_storage!(&f, &denied, actor, &f.policy);
    let allowed = SendMessage::new(
        f.alice.clone(),
        reference(&f.a)?,
        IdempotencyKey::new("base_metadata_grant")?,
        MessageId::new("base_metadata_message")?,
        recipient.clone(),
        CollaborationRef::new("shared")?,
        sealed.content().clone(),
        None,
        f.clock.now(),
    );
    let owner = f
        .policy
        .grants
        .lock()
        .map_err(|_| PeerPortError::Unavailable)?
        .remove(recipient.as_str())
        .ok_or(PeerPortError::Integrity)?;
    denied_before_storage!(&f, &allowed, actor, &f.policy);
    assert!(
        f.policy
            .grants
            .lock()
            .map_err(|_| PeerPortError::Unavailable)?
            .insert(recipient.as_str().to_owned(), owner)
            .is_none()
    );
    let before = storage_calls(&f);
    CommandExecution::new(&f.store, &allowed)
        .with_principal(actor)
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before.1 + 1);
    let persisted = history(&f, &sender).await?;
    assert_eq!(persisted.outbox.len(), 1);
    let metadata = persisted.outbox[0]
        .message
        .as_option()
        .ok_or(PeerPortError::Integrity)?;
    assert_eq!(metadata.message_id.as_deref(), Some("base_metadata_message"));
    assert_eq!(metadata.recipient_peer_id.as_deref(), Some(recipient.as_str()));
    assert_eq!(metadata.collaboration_ref.as_deref(), Some("shared"));
    assert_eq!(
        metadata.content_ref.as_option(),
        Some(&sealed.content().reference().to_claim_check())
    );
    assert_eq!(metadata.content_size_bytes, Some(sealed.content().size()));
    assert_eq!(
        metadata.semantic_fingerprint.value.as_deref(),
        Some(sealed.content().semantic_fingerprint())
    );
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn public_fixture_batch_id_grants_reject_preparation_and_ack_before_storage() -> TestResult {
    let f = Fixture::new().await?;
    let accepted = accepted(&f, "base_batch_grants").await?;
    let actor = principal(&f.bob)?;
    CommandExecution::new(&f.store, &IntakeMessage::new(accepted.evidence.clone(), f.clock.now()))
        .with_principal(actor.clone())
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let batch = BatchId::new("base_permitted_batch")?;
    let draft = proto::PeerBatch {
        batch_id: Some(batch.as_str().to_owned()),
        message_ids: vec![accepted.message.as_str().to_owned()],
        prepared_at: trogonai_proto::convert::timestamp_from_datetime(&f.clock.now()).into(),
        ..Default::default()
    };
    let rendered = trogon_peer_context::PeerContext::render(
        &draft,
        &[proto::PreparedMessage {
            metadata: accepted.evidence.metadata().clone().into(),
            text: Some("payload".into()),
        }],
    )?;
    let plan = TrustedBatchPlan::from_renderer(vec![accepted.message.clone()], rendered.as_str().len())?;
    let denied = PrepareBatch::new(
        f.bob.clone(),
        reference(&f.b)?,
        BatchId::new("denied")?,
        plan.clone(),
        f.clock.now(),
    );
    denied_before_storage!(&f, &denied, actor, &f.policy);
    let allowed = PrepareBatch::new(f.bob.clone(), reference(&f.b)?, batch.clone(), plan, f.clock.now());
    CommandExecution::new(&f.store, &allowed)
        .with_principal(actor.clone())
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    let prepared = history(&f, &accepted.recipient).await?;
    assert_eq!(prepared.batches.len(), 1);
    assert_eq!(prepared.batches[0].batch_id.as_deref(), Some(batch.as_str()));
    assert_eq!(
        prepared.batches[0].message_ids,
        vec![accepted.message.as_str().to_owned()]
    );
    assert!(!prepared.batches[0].acknowledged_at.is_set());
    let denied = AcknowledgeBatch::new(f.bob.clone(), reference(&f.b)?, BatchId::new("denied")?, f.clock.now());
    denied_before_storage!(&f, &denied, actor, &f.policy);
    let allowed = AcknowledgeBatch::new(f.bob.clone(), reference(&f.b)?, batch, f.clock.now());
    let before = storage_calls(&f);
    CommandExecution::new(&f.store, &allowed)
        .with_principal(actor)
        .with_authorizer(&f.policy)
        .with_replay_limit(ReplayLimit::try_new(4096)?)
        .execute()
        .await?;
    assert_eq!(f.store.appends.load(Ordering::SeqCst), before.1 + 1);
    let acknowledged = history(&f, &accepted.recipient).await?;
    assert!(acknowledged.batches[0].acknowledged_at.is_set());
    assert_eq!(acknowledged.batches[0].message_ids, prepared.batches[0].message_ids);
    assert_eq!(f.cache.calls.load(Ordering::SeqCst), 0);
    Ok(())
}
