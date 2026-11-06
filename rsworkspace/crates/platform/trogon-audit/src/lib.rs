#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]

//! # trogon-audit
//!
//! The platform audit record's subjects, class, sink and writer
//! (ADR#0086 Decisions 2 to 5).
//!
//! - [`AuditSubject`] builds `platform.v1.audit.{kind}.{scope}.{terminal}` from
//!   validated tokens and closed terminal vocabularies.
//! - [`Record`] is built from facts, and [`RecordKind::audit_class`] fixes its
//!   class. No caller chooses a class.
//! - [`AuditSink`] is the port; [`JetStreamSink`] publishes with `Nats-Msg-Id`
//!   set to the record id and returns the stream's [`StoredAck`].
//! - [`AuditWriter`] refuses an action whose must-audit record it cannot store,
//!   buffers the record of a refusal, and queues best-effort records. It stores
//!   every record as a link of one gap-free hash chain per writer instance,
//!   opened by a chain reset under its [`WriterIdentity`], and reports the last
//!   acknowledged link as its [`ChainHead`].
//!
//! Records hold no secret, token, prompt, tool argument, header value or body:
//! every string is an [`AuditName`], a [`ReasonCode`] or an opaque
//! [`ConsistencyToken`], and no API takes content.
//!
//! The per-writer hash chain of Decision 6 has its primitives here: [`RecordHash`],
//! [`ChainId`] and the stored form in [`record_seal`]. [`verify_chain_range`]
//! checks one chain read back from the stream, offline, and [`verify_chains`]
//! splits a range that interleaves writers into its chains.
//!
//! A chain cannot show that its tail was deleted, so a [`CheckpointWriter`]
//! signs the writer's acknowledged head as a detached JWS through a
//! [`CheckpointSigner`] and stores it on `AUDIT_CHECKPOINT` through a
//! [`CheckpointSink`]. [`verify_checkpoint`] checks one against a
//! [`VerifiedChainHead`] under a set of [`TrustedKeys`]. Resuming a chain across
//! restarts, a key-management signer, the remaining integrity records, the
//! OpenTelemetry counters and the surface adapters are later slices.

pub mod actor;
pub mod audit_name;
pub mod audit_sink;
pub mod audit_subject;
pub mod audit_unavailable_error;
pub mod audit_writer;
pub mod chain_anchor;
pub mod chain_head;
pub mod chain_id;
pub mod chain_key;
pub mod chain_notice;
pub mod chain_outcome;
pub(crate) mod chain_tip;
pub mod chain_verification_error;
pub(crate) mod chain_walk;
pub mod chains_verification;
pub mod change_facts;
pub mod change_terminal;
pub mod checkpoint_config;
pub mod checkpoint_draft;
pub mod checkpoint_id;
pub(crate) mod checkpoint_jws;
pub mod checkpoint_sign_error;
pub mod checkpoint_signature;
pub mod checkpoint_signer;
pub mod checkpoint_sink;
pub mod checkpoint_stats;
pub mod checkpoint_terminal;
pub mod checkpoint_verification_error;
pub mod checkpoint_writer;
pub mod classify_publish_error;
pub mod consistency_token;
pub mod constants;
pub mod credential_facts;
pub mod credential_terminal;
pub mod decision_facts;
pub mod decision_outcome;
pub mod decision_terminal;
pub mod disposition;
pub mod enforcement;
pub mod failure_reason;
pub(crate) mod head_watch;
pub mod integrity_terminal;
pub mod jetstream_sink;
pub mod key_id;
pub mod layer_facts;
pub mod local_checkpoint_signer;
pub mod obligation;
pub(crate) mod pending_record;
pub(crate) mod protected_header;
pub mod read_only;
pub mod reason_code;
pub mod record;
pub mod record_hash;
pub mod record_id;
pub mod record_kind;
pub mod record_scope;
pub mod record_seal;
pub mod seal_error;
pub mod sealed_record;
pub mod signed_checkpoint;
pub mod sink_error;
pub mod stamp;
pub mod stamper;
pub mod stored_ack;
pub mod stored_audit_record;
pub(crate) mod subject_agreement;
pub mod surface;
pub mod time_bounds;
pub mod tls_mode;
pub mod trace_id;
pub mod trusted_keys;
pub mod verified_chain_head;
pub mod verified_chain_range;
pub mod verified_checkpoint;
pub mod verify_chain_range;
pub mod verify_chains;
pub mod verify_checkpoint;
pub mod writer_config;
pub mod writer_id;
pub mod writer_identity;
pub(crate) mod writer_state;
pub mod writer_stats;

#[cfg(any(test, feature = "test-support"))]
pub mod chain_builder;
#[cfg(any(test, feature = "test-support"))]
pub mod memory_sink;

pub use actor::Actor;
pub use audit_name::{AuditName, AuditNameError};
pub use audit_sink::AuditSink;
pub use audit_subject::AuditSubject;
pub use audit_unavailable_error::AuditUnavailableError;
pub use audit_writer::AuditWriter;
pub use chain_anchor::ChainAnchor;
pub use chain_head::ChainHead;
pub use chain_id::{ChainId, ChainIdError};
pub use chain_key::ChainKey;
pub use chain_notice::ChainNotice;
pub use chain_outcome::ChainOutcome;
pub use chain_verification_error::ChainVerificationError;
pub use chains_verification::ChainsVerification;
pub use change_facts::ChangeFacts;
pub use change_terminal::ChangeTerminal;
pub use checkpoint_config::CheckpointConfig;
pub use checkpoint_draft::CheckpointDraft;
pub use checkpoint_id::{CheckpointId, CheckpointIdError};
pub use checkpoint_sign_error::CheckpointSignError;
pub use checkpoint_signature::CheckpointSignature;
pub use checkpoint_signer::CheckpointSigner;
pub use checkpoint_sink::CheckpointSink;
pub use checkpoint_stats::CheckpointStats;
pub use checkpoint_terminal::CheckpointTerminal;
pub use checkpoint_verification_error::CheckpointVerificationError;
pub use checkpoint_writer::CheckpointWriter;
pub use classify_publish_error::ClassifyPublishError;
pub use consistency_token::{ConsistencyToken, ConsistencyTokenError};
pub use credential_facts::CredentialFacts;
pub use credential_terminal::CredentialTerminal;
pub use decision_facts::DecisionFacts;
pub use decision_outcome::DecisionOutcome;
pub use decision_terminal::DecisionTerminal;
pub use disposition::Disposition;
pub use enforcement::Enforcement;
pub use failure_reason::FailureReason;
pub use integrity_terminal::IntegrityTerminal;
pub use jetstream_sink::JetStreamSink;
pub use key_id::{KeyId, KeyIdError};
pub use layer_facts::LayerFacts;
pub use local_checkpoint_signer::LocalCheckpointSigner;
pub use obligation::Obligation;
pub use read_only::ReadOnly;
pub use reason_code::{ReasonCode, ReasonCodeError};
pub use record::Record;
pub use record_hash::{RecordHash, RecordHashError};
pub use record_id::{RecordId, RecordIdError};
pub use record_kind::RecordKind;
pub use record_scope::{ActChainTooLongError, RecordScope};
pub use seal_error::SealError;
pub use sealed_record::SealedRecord;
pub use signed_checkpoint::SignedCheckpoint;
pub use sink_error::SinkError;
pub use stamp::Stamp;
pub use stamper::Stamper;
pub use stored_ack::StoredAck;
pub use stored_audit_record::StoredAuditRecord;
pub use surface::Surface;
pub use time_bounds::TimeBounds;
pub use tls_mode::TlsMode;
pub use trace_id::{TraceId, TraceIdError};
pub use trusted_keys::TrustedKeys;
pub use verified_chain_head::VerifiedChainHead;
pub use verified_chain_range::VerifiedChainRange;
pub use verified_checkpoint::VerifiedCheckpoint;
pub use verify_chain_range::verify_chain_range;
pub use verify_chains::verify_chains;
pub use verify_checkpoint::verify_checkpoint;
pub use writer_config::WriterConfig;
pub use writer_id::WriterId;
pub use writer_identity::WriterIdentity;
pub use writer_stats::WriterStats;

#[cfg(any(test, feature = "test-support"))]
pub use chain_builder::ChainBuilder;
#[cfg(any(test, feature = "test-support"))]
pub use memory_sink::MemorySink;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod redaction_canary;
