//! Shared builders for the unit tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use buffa::Message;
use bytes::Bytes;
use trogon_nats::NatsToken;
use trogon_std::NowV7;
use trogon_std::time::FixedEpochClock;
use trogonai_proto::platform::audit::v1::AuditRecord;
use uuid::{Builder, Uuid};

use crate::actor::Actor;
use crate::audit_name::AuditName;
use crate::chain_builder::ChainBuilder;
use crate::chain_id::ChainId;
use crate::change_facts::ChangeFacts;
use crate::change_terminal::ChangeTerminal;
use crate::constants::{HEADER_CHAIN, HEADER_SEQUENCE, HEADER_WRITER};
use crate::credential_facts::CredentialFacts;
use crate::credential_terminal::CredentialTerminal;
use crate::decision_facts::DecisionFacts;
use crate::decision_outcome::DecisionOutcome;
use crate::enforcement::Enforcement;
use crate::read_only::ReadOnly;
use crate::record::Record;
use crate::record_scope::RecordScope;
use crate::record_seal;
use crate::stamper::Stamper;
use crate::stored_audit_record::StoredAuditRecord;
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

pub(crate) const CHAIN: &str = "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f01";
pub(crate) const OTHER_CHAIN: &str = "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f02";

pub(crate) fn chain_id(value: &str) -> ChainId {
    ChainId::parse(value).unwrap()
}

pub(crate) fn chain_builder_for(writer: &str, chain: &str) -> ChainBuilder {
    ChainBuilder::new(WriterId::new(writer).unwrap(), chain_id(chain), token("egress_proxy"))
}

pub(crate) fn chain_builder() -> ChainBuilder {
    chain_builder_for("gateway-1", CHAIN)
}

/// A genesis and `length - 1` decisions at stream sequences `1..=length`.
pub(crate) fn valid_chain(builder: &mut ChainBuilder, length: u64) -> Vec<StoredAuditRecord> {
    let stamper = stamper();
    let mut records = vec![builder.chain_reset(stamper.stamp(), 1)];
    for stream_sequence in 2..=length {
        records.push(builder.record(deny(&stamper), stream_sequence));
    }
    records
}

/// The headers a stored record was read with.
pub(crate) fn stored_headers(record: &StoredAuditRecord) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let message_id = NATS_MESSAGE_ID;
    let present = [
        (message_id.as_ref(), record.message_id()),
        (HEADER_WRITER, record.writer()),
        (HEADER_SEQUENCE, record.sequence()),
        (HEADER_CHAIN, record.chain()),
    ];
    for (name, value) in present {
        if let Some(value) = value {
            headers.insert(name, value);
        }
    }
    headers
}

/// The same stored message under other metadata, a payload or a position.
pub(crate) fn restored(
    record: &StoredAuditRecord,
    stream_sequence: u64,
    payload: Bytes,
    headers: &HeaderMap,
) -> StoredAuditRecord {
    let stored = StoredAuditRecord::new(stream_sequence, payload).with_headers(headers);
    match record.subject() {
        Some(subject) => stored.with_subject(subject),
        None => stored,
    }
}

/// An identical copy of a stored message, stored again at `stream_sequence`.
pub(crate) fn stored_again(record: &StoredAuditRecord, stream_sequence: u64) -> StoredAuditRecord {
    restored(
        record,
        stream_sequence,
        record.payload().clone(),
        &stored_headers(record),
    )
}

pub(crate) fn with_payload(record: &StoredAuditRecord, payload: impl Into<Bytes>) -> StoredAuditRecord {
    restored(
        record,
        record.stream_sequence(),
        payload.into(),
        &stored_headers(record),
    )
}

/// The record's hashed body, decoded.
pub(crate) fn body(record: &StoredAuditRecord) -> AuditRecord {
    let (body, _) = record_seal::open(record.payload()).unwrap();
    AuditRecord::decode_from_slice(body).unwrap()
}

/// `message` sealed afresh, with the record's metadata kept: a record an
/// attacker rewrote and rehashed.
pub(crate) fn resealed(record: &StoredAuditRecord, message: &AuditRecord) -> StoredAuditRecord {
    let (_, payload) = record_seal::seal(Bytes::from(message.encode_to_vec()));
    with_payload(record, payload)
}
