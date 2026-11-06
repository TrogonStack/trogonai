use std::time::Duration;

/// The fixed leading subject token of the audit family (ADR#0086 Decision 2).
///
/// The family spans the A2A gateway, the egress proxy and the secrets service, so
/// no single protocol token fits. The tenant leads inside the `PLATFORM` account
/// and is deliberately absent from the subject.
pub const SUBJECT_PREFIX: &str = "platform";
pub const SUBJECT_BINDING_VERSION: &str = "v1";
pub const SUBJECT_ROOT: &str = "audit";

pub const SUBJECT_KIND_DECISION: &str = "decision";
pub const SUBJECT_KIND_CREDENTIAL: &str = "credential";
pub const SUBJECT_KIND_CHANGE: &str = "change";
pub const SUBJECT_KIND_INTEGRITY: &str = "integrity";
pub const SUBJECT_KIND_CHECKPOINT: &str = "checkpoint";

pub const TERMINAL_ALLOW: &str = "allow";
pub const TERMINAL_DENY: &str = "deny";
pub const TERMINAL_WOULD_DENY: &str = "would_deny";
pub const TERMINAL_RELEASE: &str = "release";
pub const TERMINAL_INJECT: &str = "inject";
pub const TERMINAL_POLICY: &str = "policy";
pub const TERMINAL_CREDENTIAL: &str = "credential";
pub const TERMINAL_START: &str = "start";
pub const TERMINAL_CHAIN_RESET: &str = "chain_reset";
pub const TERMINAL_SIGNED: &str = "signed";

/// Provenance travels in headers, never in payload fields (ADR#0039).
pub const HEADER_WRITER: &str = "Trogon-Audit-Writer";
pub const HEADER_SEQUENCE: &str = "Trogon-Audit-Seq";
pub const HEADER_CHAIN: &str = "Trogon-Audit-Chain";

/// Longest name a record may carry. A target, a resource name or a host is a
/// name; content does not fit in this and is rejected on character set first.
pub const MAX_NAME_BYTES: usize = 256;
pub const MAX_REASON_CODE_BYTES: usize = 64;
/// Longest snapshot token a layer verdict may carry, also the proto bound.
pub const MAX_CONSISTENCY_TOKEN_BYTES: usize = 512;

/// `MAX_CHAIN_DEPTH` of `trogon-aauth-verify`, which is also the proto bound.
pub const MAX_ACT_CHAIN_ENTRIES: usize = 16;

pub const TRACE_ID_HEX_LEN: usize = 32;

/// Characters beyond ASCII alphanumerics that a name may hold. Whitespace, quotes,
/// `=`, `,`, `;` and control characters are absent on purpose: they are what
/// separates a header, a query string or a body from a name.
pub const NAME_PUNCTUATION: &str = "._:/@+-[]";

/// Characters beyond ASCII alphanumerics an opaque snapshot token may hold: the
/// base64 and base64url alphabets with their padding, plus `.`, `:` and `~`.
pub const CONSISTENCY_TOKEN_PUNCTUATION: &str = "+/=-_.:~";

pub const RECORD_ID_UUID_VERSION: usize = 7;
pub const CHAIN_ID_UUID_VERSION: usize = 7;

/// A record hash is a SHA-256 digest.
pub const RECORD_HASH_BYTES: usize = 32;

const PROTOBUF_WIRE_TYPE_LENGTH_DELIMITED: u8 = 2;
/// `AuditRecord.integrity`.
const AUDIT_RECORD_INTEGRITY_FIELD: u8 = 12;
/// `Integrity.record_hash`.
const INTEGRITY_RECORD_HASH_FIELD: u8 = 4;
/// The record hash as a length-delimited `Integrity.record_hash`: tag and length.
const RECORD_HASH_FIELD_HEADER_BYTES: usize = 2;

/// The bytes that open a sealed record's trailer: a second `AuditRecord.integrity`
/// holding only `record_hash`, as one-byte tags and one-byte lengths. A decoder
/// merges it into the record's integrity.
pub const SEAL_TRAILER_PREFIX: [u8; 4] = [
    (AUDIT_RECORD_INTEGRITY_FIELD << 3) | PROTOBUF_WIRE_TYPE_LENGTH_DELIMITED,
    (RECORD_HASH_FIELD_HEADER_BYTES + RECORD_HASH_BYTES) as u8,
    (INTEGRITY_RECORD_HASH_FIELD << 3) | PROTOBUF_WIRE_TYPE_LENGTH_DELIMITED,
    RECORD_HASH_BYTES as u8,
];
pub const SEAL_TRAILER_BYTES: usize = SEAL_TRAILER_PREFIX.len() + RECORD_HASH_BYTES;

/// A lost ack is retried inside the stream's duplicate window (two minutes), so
/// three attempts of a couple of seconds stay well within it.
pub const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_MAX_PUBLISH_ATTEMPTS: u32 = 3;

pub const DEFAULT_BEST_EFFORT_CAPACITY: usize = 1024;
pub const DEFAULT_RETRY_BUFFER_CAPACITY: usize = 256;
pub const DEFAULT_DRAIN_INTERVAL: Duration = Duration::from_secs(1);

/// Target of the structured local log lines the writer emits when it refuses an
/// action or drops a record.
pub const LOG_TARGET: &str = "trogon_audit";
