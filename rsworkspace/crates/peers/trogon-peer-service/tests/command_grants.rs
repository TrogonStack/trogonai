use std::sync::atomic::Ordering;

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
