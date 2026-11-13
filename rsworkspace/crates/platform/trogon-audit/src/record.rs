//! An audit record before a writer has sealed it.
//!
//! A record is built from facts, and its class, subject and obligation fall out of
//! them. There is no setter for the class, so a caller cannot make a refusal
//! best-effort or a forward cheap.

use buffa::{EnumValue, Inline, MessageField};
use buffa_types::google::protobuf::{Duration as ProtoDuration, Timestamp};
use trogonai_proto::agents::agents::v1alpha1::AgentId;
use trogonai_proto::hierarchy::v1alpha1::NodeId;
use trogonai_proto::platform::audit::v1::audit_actor::{
    AgentActor, ApiKeyActor, BreakGlassActor, HumanSessionActor, ServiceActor, Unattributed,
};
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{
    ActChainEntry, Attribution, AuditActor, AuditDecision, AuditRecord, ChainResetReason, Change, ChangeKind,
    CredentialUse, CredentialUseKind, EnforcementMode, GatewayDecision, GatewaySurface, LayerVerdict,
    TlsMode as ProtoTlsMode, WriterLifecycle, WriterLifecycleKind,
};
use trogonai_proto::projects::projects::v1alpha1::ProjectId;
use trogonai_proto::session::sessions::v1alpha1::SessionId;

use crate::actor::Actor;
use crate::audit_subject::AuditSubject;
use crate::change_facts::ChangeFacts;
use crate::change_terminal::ChangeTerminal;
use crate::credential_activation_facts::{ActivationScopeError, CredentialActivationFacts};
use crate::credential_facts::CredentialFacts;
use crate::credential_terminal::CredentialTerminal;
use crate::decision_facts::DecisionFacts;
use crate::decision_outcome::DecisionOutcome;
use crate::enforcement::Enforcement;
use crate::integrity_terminal::IntegrityTerminal;
use crate::layer_facts::LayerFacts;
use crate::obligation::Obligation;
use crate::read_only::ReadOnly;
use crate::record_id::RecordId;
use crate::record_kind::RecordKind;
use crate::record_scope::RecordScope;
use crate::stamp::Stamp;
use crate::surface::Surface;
use crate::tls_mode::TlsMode;
use crate::workload_peer_facts::{WorkloadPeerEvidenceFacts, WorkloadPeerScopeError};
use crate::writer_identity::WriterIdentity;

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    id: RecordId,
    subject: AuditSubject,
    kind: RecordKind,
    message: AuditRecord,
}

impl Record {
    pub fn authenticated_workload_decision(
        scope: RecordScope,
        facts: DecisionFacts,
        peer: WorkloadPeerEvidenceFacts,
        stamp: Stamp,
    ) -> Result<Self, WorkloadPeerScopeError> {
        if facts.surface != Surface::Egress
            || facts.read_only != ReadOnly::Undeclared
            || facts.outcome.enforcement() != Enforcement::Enforce
        {
            return Err(WorkloadPeerScopeError);
        }
        let Actor::Agent { agent_id } = &scope.actor else {
            return Err(WorkloadPeerScopeError);
        };
        if !peer.matches_binding(scope.tenant.as_str(), scope.project.as_str(), agent_id.as_str())
            || (!facts.outcome.is_refusal() && !peer.valid_at(&Timestamp::from(stamp.occurred_at)))
        {
            return Err(WorkloadPeerScopeError);
        }
        Ok(Self::decision_with_peer(scope, facts, Some(peer), stamp))
    }

    pub fn credential_activation(
        scope: RecordScope,
        facts: CredentialActivationFacts,
        stamp: Stamp,
    ) -> Result<Self, ActivationScopeError> {
        if !facts.matches_scope(scope.tenant.as_str(), scope.project.as_str()) {
            return Err(ActivationScopeError);
        }
        let subject = AuditSubject::change(scope.project.clone(), ChangeTerminal::Credential);
        Ok(Self::assemble(
            scope,
            stamp,
            RecordKind::CredentialActivation,
            subject,
            credential_attribution(),
            Detail::from(facts.into_message()),
        ))
    }

    pub fn decision(scope: RecordScope, facts: DecisionFacts, stamp: Stamp) -> Self {
        Self::decision_with_peer(scope, facts, None, stamp)
    }

    fn decision_with_peer(
        scope: RecordScope,
        facts: DecisionFacts,
        peer: Option<WorkloadPeerEvidenceFacts>,
        stamp: Stamp,
    ) -> Self {
        let kind = RecordKind::Decision {
            outcome: facts.outcome,
            read_only: facts.read_only,
            break_glass: scope.actor.is_break_glass(),
        };
        let subject = AuditSubject::decision(scope.project.clone(), facts.outcome.terminal());
        let attribution = attribution(&facts);
        let detail = Detail::from(gateway_decision(&facts, peer));
        Self::assemble(scope, stamp, kind, subject, attribution, detail)
    }

    pub fn credential_use(scope: RecordScope, facts: CredentialFacts, stamp: Stamp) -> Self {
        let subject = AuditSubject::credential(scope.project.clone(), facts.kind);
        let attribution = credential_attribution();
        let detail = Detail::from(credential_use(&facts));
        Self::assemble(scope, stamp, RecordKind::CredentialUse, subject, attribution, detail)
    }

    pub fn change(scope: RecordScope, facts: ChangeFacts, stamp: Stamp) -> Self {
        let subject = AuditSubject::change(scope.project.clone(), facts.kind);
        let detail = Detail::from(change(&facts));
        Self::assemble(scope, stamp, RecordKind::Change, subject, MessageField::none(), detail)
    }

    /// The record that opens a writer's chain. A writer starts a new chain every
    /// time it starts and does not resume the last one, so its first record says
    /// so. Only the writer builds it.
    pub(crate) fn chain_reset(identity: &WriterIdentity, stamp: Stamp) -> Self {
        let subject = AuditSubject::integrity(identity.component().clone(), IntegrityTerminal::ChainReset);
        let detail = Detail::from(WriterLifecycle {
            kind: EnumValue::from(WriterLifecycleKind::ChainReset),
            reset_reason: Some(EnumValue::from(ChainResetReason::ProcessStartWithoutResume)),
        });
        Self::assemble(
            identity.scope(),
            stamp,
            RecordKind::Integrity,
            subject,
            MessageField::none(),
            detail,
        )
    }

    pub(crate) fn readiness_probe(identity: &WriterIdentity, stamp: Stamp) -> Self {
        let subject = AuditSubject::integrity(identity.component().clone(), IntegrityTerminal::ReadinessProbe);
        Self::assemble(
            identity.scope(),
            stamp,
            RecordKind::Integrity,
            subject,
            MessageField::none(),
            Detail::from(WriterLifecycle {
                kind: EnumValue::from(WriterLifecycleKind::ReadinessProbe),
                reset_reason: None,
            }),
        )
    }

    fn assemble(
        scope: RecordScope,
        stamp: Stamp,
        kind: RecordKind,
        subject: AuditSubject,
        attribution: MessageField<Attribution, Inline<Attribution>>,
        detail: Detail,
    ) -> Self {
        let message = AuditRecord {
            record_id: stamp.id.to_string(),
            class: EnumValue::from(kind.audit_class()),
            occurred_at: MessageField::some(Timestamp::from(stamp.occurred_at)),
            recorded_at: MessageField::some(Timestamp::from(stamp.recorded_at)),
            tenant: MessageField::some(NodeId {
                value: scope.tenant.to_string(),
            }),
            project: MessageField::some(ProjectId {
                value: scope.project.to_string(),
            }),
            actor: MessageField::some(audit_actor(&scope.actor)),
            act_chain: scope
                .act_chain
                .iter()
                .map(|agent| ActChainEntry {
                    agent_id: MessageField::some(agent_id(agent.as_str())),
                })
                .collect(),
            trace_id: scope.trace_id.map(|trace_id| trace_id.as_str().to_owned()),
            request_id: scope.request_id.map(|request_id| request_id.to_string()),
            attribution,
            detail: Some(detail),
            ..Default::default()
        };
        Self {
            id: stamp.id,
            subject,
            kind,
            message,
        }
    }

    /// The decision identifier a surface hands back to its caller, and the
    /// `Nats-Msg-Id` the stream deduplicates on.
    pub fn id(&self) -> RecordId {
        self.id
    }

    pub fn subject(&self) -> &AuditSubject {
        &self.subject
    }

    pub fn kind(&self) -> RecordKind {
        self.kind
    }

    pub fn obligation(&self) -> Obligation {
        self.kind.obligation()
    }

    pub fn message(&self) -> &AuditRecord {
        &self.message
    }

    pub(crate) fn into_message(self) -> (RecordId, AuditSubject, RecordKind, AuditRecord) {
        (self.id, self.subject, self.kind, self.message)
    }
}

fn agent_id(value: &str) -> AgentId {
    AgentId {
        value: value.to_owned(),
    }
}

fn audit_actor(actor: &Actor) -> AuditActor {
    let kind = match actor {
        Actor::Unattributed => Unattributed::default().into(),
        Actor::ApiKey { api_key_id } => ApiKeyActor {
            api_key_id: api_key_id.to_string(),
        }
        .into(),
        Actor::HumanSession { subject, issuer } => HumanSessionActor {
            subject: subject.to_string(),
            issuer: issuer.to_string(),
        }
        .into(),
        Actor::BreakGlass { operator, host } => BreakGlassActor {
            operator: operator.to_string(),
            host: host.to_string(),
        }
        .into(),
        Actor::Agent { agent_id: id } => AgentActor {
            agent_id: MessageField::some(agent_id(id.as_str())),
        }
        .into(),
        Actor::Service { service_id } => ServiceActor {
            service_id: service_id.to_string(),
        }
        .into(),
    };
    AuditActor { kind }
}

fn audit_decision(outcome: DecisionOutcome) -> AuditDecision {
    match outcome {
        DecisionOutcome::Allow(_) | DecisionOutcome::WouldDeny => AuditDecision::AUDIT_DECISION_ALLOW,
        DecisionOutcome::Deny => AuditDecision::AUDIT_DECISION_DENY,
    }
}

fn enforcement_mode(enforcement: Enforcement) -> EnforcementMode {
    match enforcement {
        Enforcement::Enforce => EnforcementMode::ENFORCEMENT_MODE_ENFORCE,
        Enforcement::DryRun => EnforcementMode::ENFORCEMENT_MODE_DRY_RUN,
    }
}

fn attribution(facts: &DecisionFacts) -> MessageField<Attribution, Inline<Attribution>> {
    MessageField::some(Attribution {
        decision: EnumValue::from(audit_decision(facts.outcome)),
        enforcement_mode: EnumValue::from(enforcement_mode(facts.outcome.enforcement())),
        would_deny: facts.outcome == DecisionOutcome::WouldDeny,
        reason_code: facts.reason.map(|reason| reason.as_str().to_owned()),
        rule_ids: facts.rule_ids.iter().map(ToString::to_string).collect(),
        policy_source: facts.policy_source.as_ref().map(ToString::to_string),
        projection_revision: facts.projection_revision,
        ..Default::default()
    })
}

/// A release or injection is an effect that happens in every mode, so its
/// attribution is an enforced allow. The envelope requires attribution on a
/// credential record.
fn credential_attribution() -> MessageField<Attribution, Inline<Attribution>> {
    MessageField::some(Attribution {
        decision: EnumValue::from(AuditDecision::AUDIT_DECISION_ALLOW),
        enforcement_mode: EnumValue::from(EnforcementMode::ENFORCEMENT_MODE_ENFORCE),
        ..Default::default()
    })
}

fn gateway_surface(surface: Surface) -> GatewaySurface {
    match surface {
        Surface::A2aIngress => GatewaySurface::GATEWAY_SURFACE_A2A_INGRESS,
        Surface::Egress => GatewaySurface::GATEWAY_SURFACE_EGRESS,
        Surface::SecretsRelease => GatewaySurface::GATEWAY_SURFACE_SECRETS_RELEASE,
    }
}

fn tls_mode(mode: TlsMode) -> ProtoTlsMode {
    match mode {
        TlsMode::Passthrough => ProtoTlsMode::TLS_MODE_PASSTHROUGH,
        TlsMode::Intercepted => ProtoTlsMode::TLS_MODE_INTERCEPTED,
    }
}

fn layer_verdict(layer: &LayerFacts) -> LayerVerdict {
    LayerVerdict {
        layer: layer.layer.to_string(),
        decision: EnumValue::from(audit_decision(layer.outcome)),
        enforcement_mode: EnumValue::from(enforcement_mode(layer.outcome.enforcement())),
        would_deny: layer.outcome == DecisionOutcome::WouldDeny,
        reason_code: layer.reason.map(|reason| reason.as_str().to_owned()),
        rule_ids: layer.rule_ids.iter().map(ToString::to_string).collect(),
        consistency_token: layer.consistency_token.as_ref().map(|token| token.as_str().to_owned()),
    }
}

fn gateway_decision(facts: &DecisionFacts, peer: Option<WorkloadPeerEvidenceFacts>) -> GatewayDecision {
    GatewayDecision {
        workload_peer: peer.map_or_else(MessageField::none, |peer| MessageField::some(peer.into_message())),
        surface: EnumValue::from(gateway_surface(facts.surface)),
        target: facts.target.to_string(),
        request_bytes: facts.request_bytes,
        response_bytes: facts.response_bytes,
        latency: facts.latency.map_or_else(MessageField::none, |latency| {
            MessageField::some(ProtoDuration::from(latency))
        }),
        tls_mode: facts.tls_mode.map(|mode| EnumValue::from(tls_mode(mode))),
        layer_verdicts: facts.layers.iter().map(layer_verdict).collect(),
        session: facts.session.as_ref().map_or_else(MessageField::none, |session| {
            MessageField::some(SessionId {
                value: session.to_string(),
            })
        }),
    }
}

fn credential_use(facts: &CredentialFacts) -> CredentialUse {
    CredentialUse {
        kind: EnumValue::from(match facts.kind {
            CredentialTerminal::Release => CredentialUseKind::CREDENTIAL_USE_KIND_RELEASE,
            CredentialTerminal::Inject => CredentialUseKind::CREDENTIAL_USE_KIND_INJECT,
        }),
        credential: facts.credential.to_string(),
        binding_id: facts.binding_id.to_string(),
        destination: facts.destination.as_ref().map(ToString::to_string),
        decision_record_id: facts.decision_record_id.map(|id| id.to_string()),
        location: facts.location.as_ref().map(ToString::to_string),
    }
}

fn change(facts: &ChangeFacts) -> Change {
    Change {
        kind: EnumValue::from(match facts.kind {
            ChangeTerminal::Policy => ChangeKind::CHANGE_KIND_POLICY,
            ChangeTerminal::Credential => ChangeKind::CHANGE_KIND_CREDENTIAL,
        }),
        resource_name: facts.resource_name.to_string(),
        event_type: facts.event_type.to_string(),
        source_stream: facts.source_stream.to_string(),
        stream_sequence: facts.stream_sequence,
        event_id: facts.event_id.to_string(),
    }
}

#[cfg(test)]
mod tests;
