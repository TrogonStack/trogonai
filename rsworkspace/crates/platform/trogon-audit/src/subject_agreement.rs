//! Whether a record's subject is the one its payload implies (ADR#0086
//! Decision 2).
//!
//! The kind and terminal follow from the detail arm, and the scope from the
//! project, except on an integrity subject, whose scope is the writer's
//! component, which the payload does not name.

use crate::credential_activation_facts::CredentialActivationFacts;
use crate::workload_peer_facts::WorkloadPeerEvidenceFacts;
use trogonai_proto::platform::audit::v1::audit_actor::Kind;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{
    AuditClass, AuditDecision, AuditRecord, ChangeKind, CredentialUseKind, EnforcementMode, GatewaySurface,
    WriterLifecycleKind,
};

use crate::constants::{
    SUBJECT_BINDING_VERSION, SUBJECT_KIND_CHANGE, SUBJECT_KIND_CREDENTIAL, SUBJECT_KIND_DECISION,
    SUBJECT_KIND_INTEGRITY, SUBJECT_PREFIX, SUBJECT_ROOT, TERMINAL_ALLOW, TERMINAL_CHAIN_RESET, TERMINAL_CREDENTIAL,
    TERMINAL_DENY, TERMINAL_INJECT, TERMINAL_POLICY, TERMINAL_READINESS_PROBE, TERMINAL_RELEASE, TERMINAL_START,
    TERMINAL_WOULD_DENY,
};

pub(crate) fn subject_agrees(subject: &str, record: &AuditRecord) -> bool {
    let Some((kind, scope, terminal)) = expected(record) else {
        return false;
    };
    let tokens: Vec<&str> = subject.split('.').collect();
    let [prefix, version, root, subject_kind, subject_scope, subject_terminal] = tokens[..] else {
        return false;
    };
    prefix == SUBJECT_PREFIX
        && version == SUBJECT_BINDING_VERSION
        && root == SUBJECT_ROOT
        && subject_kind == kind
        && scope.is_none_or(|scope| subject_scope == scope)
        && subject_terminal == terminal
}

fn expected(record: &AuditRecord) -> Option<(&'static str, Option<&str>, &'static str)> {
    let project = Some(record.project.value.as_str());
    match record.detail.as_ref()? {
        Detail::CredentialActivation(activation) => {
            let facts = CredentialActivationFacts::try_from(activation.as_ref().clone()).ok()?;
            let attribution = record.attribution.as_option()?;
            if !facts.matches_scope(&record.tenant.value, &record.project.value)
                || record.class.as_known()? != AuditClass::Must
                || attribution.decision.as_known()? != AuditDecision::AUDIT_DECISION_ALLOW
                || attribution.enforcement_mode.as_known()? != EnforcementMode::ENFORCEMENT_MODE_ENFORCE
                || attribution.would_deny
            {
                return None;
            }
            Some((SUBJECT_KIND_CHANGE, project, TERMINAL_CREDENTIAL))
        }
        Detail::GatewayDecision(decision) => {
            if let Some(peer) = decision.workload_peer.as_option() {
                let facts = WorkloadPeerEvidenceFacts::try_from(peer.clone()).ok()?;
                let attribution = record.attribution.as_option()?;
                let Some(Kind::Agent(actor)) = record.actor.kind.as_ref() else {
                    return None;
                };
                if record.class.as_known()? != AuditClass::Must
                    || decision.surface.as_known()? != GatewaySurface::GATEWAY_SURFACE_EGRESS
                    || attribution.enforcement_mode.as_known()? != EnforcementMode::ENFORCEMENT_MODE_ENFORCE
                    || attribution.would_deny
                    || (attribution.decision.as_known()? == AuditDecision::AUDIT_DECISION_ALLOW
                        && !record
                            .occurred_at
                            .as_option()
                            .is_some_and(|occurred| facts.valid_at(occurred)))
                    || !facts.matches_binding(&record.tenant.value, &record.project.value, &actor.agent_id.value)
                {
                    return None;
                }
            }
            Some((SUBJECT_KIND_DECISION, project, decision_terminal(record)?))
        }
        Detail::CredentialUse(credential) => {
            let terminal = match credential.kind.as_known()? {
                CredentialUseKind::CREDENTIAL_USE_KIND_RELEASE => TERMINAL_RELEASE,
                CredentialUseKind::CREDENTIAL_USE_KIND_INJECT => TERMINAL_INJECT,
                CredentialUseKind::CREDENTIAL_USE_KIND_UNSPECIFIED => return None,
            };
            Some((SUBJECT_KIND_CREDENTIAL, project, terminal))
        }
        Detail::Change(change) => {
            let terminal = match change.kind.as_known()? {
                ChangeKind::CHANGE_KIND_POLICY => TERMINAL_POLICY,
                ChangeKind::CHANGE_KIND_CREDENTIAL => TERMINAL_CREDENTIAL,
                ChangeKind::CHANGE_KIND_UNSPECIFIED => return None,
            };
            Some((SUBJECT_KIND_CHANGE, project, terminal))
        }
        Detail::WriterLifecycle(lifecycle) => {
            let terminal = match lifecycle.kind.as_known()? {
                WriterLifecycleKind::WRITER_LIFECYCLE_KIND_START => TERMINAL_START,
                WriterLifecycleKind::WRITER_LIFECYCLE_KIND_CHAIN_RESET => TERMINAL_CHAIN_RESET,
                WriterLifecycleKind::WRITER_LIFECYCLE_KIND_READINESS_PROBE => {
                    if record.class.as_known()? != AuditClass::Must
                        || record.attribution.as_option().is_some()
                        || lifecycle.reset_reason.is_some()
                    {
                        return None;
                    }
                    TERMINAL_READINESS_PROBE
                }
                WriterLifecycleKind::WRITER_LIFECYCLE_KIND_UNSPECIFIED => return None,
            };
            Some((SUBJECT_KIND_INTEGRITY, None, terminal))
        }
    }
}

/// A DryRun refusal is recorded as an allow that would have been denied.
fn decision_terminal(record: &AuditRecord) -> Option<&'static str> {
    let attribution = record.attribution.as_option()?;
    if attribution.would_deny {
        return Some(TERMINAL_WOULD_DENY);
    }
    match attribution.decision.as_known()? {
        AuditDecision::AUDIT_DECISION_ALLOW => Some(TERMINAL_ALLOW),
        AuditDecision::AUDIT_DECISION_DENY => Some(TERMINAL_DENY),
        AuditDecision::AUDIT_DECISION_UNSPECIFIED => None,
    }
}

#[cfg(test)]
mod tests;
