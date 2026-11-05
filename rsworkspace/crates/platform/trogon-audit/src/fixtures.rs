//! Shared builders for the unit tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use trogon_nats::NatsToken;
use trogon_std::NowV7;
use trogon_std::time::FixedEpochClock;
use uuid::{Builder, Uuid};

use crate::actor::Actor;
use crate::audit_name::AuditName;
use crate::change_facts::ChangeFacts;
use crate::change_terminal::ChangeTerminal;
use crate::credential_facts::CredentialFacts;
use crate::credential_terminal::CredentialTerminal;
use crate::decision_facts::DecisionFacts;
use crate::decision_outcome::DecisionOutcome;
use crate::enforcement::Enforcement;
use crate::read_only::ReadOnly;
use crate::record::Record;
use crate::record_scope::RecordScope;
use crate::stamper::Stamper;
use crate::surface::Surface;
use crate::writer_id::WriterId;

/// Yields distinct UUIDv7 values, one more millisecond apart each call.
#[derive(Debug, Clone, Default)]
pub(crate) struct SequentialIds(Arc<AtomicU64>);

impl NowV7 for SequentialIds {
    fn now_v7(&self) -> Uuid {
        let millis = self.0.fetch_add(1, Ordering::Relaxed) + 1_700_000_000_000;
        Builder::from_unix_timestamp_millis(millis, &[0; 10]).into_uuid()
    }
}

pub(crate) fn name(value: &str) -> AuditName {
    AuditName::new(value).unwrap()
}

pub(crate) fn token(value: &str) -> NatsToken {
    NatsToken::new(value).unwrap()
}

pub(crate) fn writer_id() -> WriterId {
    WriterId::new("gateway-1").unwrap()
}

pub(crate) fn stamper() -> Stamper<FixedEpochClock, SequentialIds> {
    Stamper::new(FixedEpochClock::from_secs(1_800_000_000), SequentialIds::default())
}

pub(crate) fn scope_for(actor: Actor) -> RecordScope {
    RecordScope::new(name("tenant_1"), token("project_1"), actor)
}

pub(crate) fn scope() -> RecordScope {
    scope_for(Actor::Agent {
        agent_id: name("agent-7"),
    })
}

pub(crate) fn decision_facts(outcome: DecisionOutcome, read_only: ReadOnly) -> DecisionFacts {
    let mut facts = DecisionFacts::new(Surface::Egress, name("api.example.com"), outcome);
    facts.read_only = read_only;
    facts
}

pub(crate) fn decision_record(stamper: &Stamper<FixedEpochClock, SequentialIds>, outcome: DecisionOutcome) -> Record {
    Record::decision(scope(), decision_facts(outcome, ReadOnly::Undeclared), stamper.stamp())
}

pub(crate) fn deny(stamper: &Stamper<FixedEpochClock, SequentialIds>) -> Record {
    decision_record(stamper, DecisionOutcome::Deny)
}

pub(crate) fn allow(stamper: &Stamper<FixedEpochClock, SequentialIds>) -> Record {
    decision_record(stamper, DecisionOutcome::Allow(Enforcement::Enforce))
}

pub(crate) fn best_effort(stamper: &Stamper<FixedEpochClock, SequentialIds>) -> Record {
    decision_record(stamper, DecisionOutcome::WouldDeny)
}

pub(crate) fn credential(stamper: &Stamper<FixedEpochClock, SequentialIds>) -> Record {
    let facts = CredentialFacts::new(CredentialTerminal::Inject, name("github_token"), name("binding_1"));
    Record::credential_use(scope(), facts, stamper.stamp())
}

pub(crate) fn change(stamper: &Stamper<FixedEpochClock, SequentialIds>) -> Record {
    let facts = ChangeFacts {
        kind: ChangeTerminal::Policy,
        resource_name: name("policies/egress"),
        event_type: name("policy_changed"),
        source_stream: name("POLICY_EVENTS"),
        stream_sequence: 42,
        event_id: name("evt_1"),
    };
    Record::change(scope(), facts, stamper.stamp())
}

pub(crate) fn server_error(code: usize, err_code: u64) -> async_nats::jetstream::Error {
    serde_json::from_value(serde_json::json!({
        "code": code,
        "err_code": err_code,
        "description": "synthetic",
    }))
    .unwrap()
}
