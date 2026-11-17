use std::time::Duration;

use trogon_std::NonZeroDuration;

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
pub const TERMINAL_READINESS_PROBE: &str = "readiness_probe";
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

pub const MAX_WORKLOAD_SPIFFE_ID_BYTES: usize = 2048;
pub const MAX_WORKLOAD_SCOPE_BYTES: usize = 128;
pub const MAX_WORKLOAD_APPLICATION_ID_BYTES: usize = 256;
pub const WORKLOAD_PUBLIC_FINGERPRINT_BYTES: usize = 32;
pub const MAX_ACTIVATION_VALIDITY_SECONDS: i64 = 3600;
pub const MAX_PROTOBUF_TIMESTAMP_SECONDS: i64 = 253_402_300_799;
pub const NANOSECONDS_PER_SECOND: i128 = 1_000_000_000;

pub const TRACE_ID_HEX_LEN: usize = 32;

/// Characters beyond ASCII alphanumerics that a name may hold. Whitespace, quotes,
/// `=`, `,`, `;` and control characters are absent on purpose: they are what
/// separates a header, a query string or a body from a name. `~` is present
/// because it is RFC 3986 unreserved, so names such as query parameters use it,
/// and it separates nothing.
pub const NAME_PUNCTUATION: &str = "._:/@+-[]~";

/// Characters beyond ASCII alphanumerics an opaque snapshot token may hold: the
/// base64 and base64url alphabets with their padding, plus `.`, `:` and `~`.
pub const CONSISTENCY_TOKEN_PUNCTUATION: &str = "+/=-_.:~";

pub const RECORD_ID_UUID_VERSION: usize = 7;
pub const CHAIN_ID_UUID_VERSION: usize = 7;
pub const CHECKPOINT_ID_UUID_VERSION: usize = 7;

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

/// The sequence of the record that opens a chain.
pub const GENESIS_SEQUENCE: u64 = 1;

/// Most records one verification walks. A verifier keeps only the previous
/// record's position and hash, so the bound limits the work one call does on
/// input it does not trust; a longer history is verified in ranges, each
/// continuing from the head the previous range verified.
pub const MAX_CHAIN_RANGE_RECORDS: usize = 10_000;

/// The NATS server's default `max_payload`, which bounds a message's payload and
/// headers together. The `AUDIT` stream sets no message size of its own, so this
/// is the size a record must fit.
pub const NATS_DEFAULT_MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Room the writer keeps for what it adds after the size check: the integrity
/// stamp (a writer id up to [`MAX_NAME_BYTES`], a sequence, a chain id and a
/// previous hash), the seal trailer and the provenance headers. Their worst case
/// is under a kilobyte.
pub const SEAL_HEADROOM_BYTES: usize = 4 * 1024;
/// Largest encoded record the writer accepts, checked before it is numbered so
/// a record the stream would reject never takes a place in the chain.
pub const MAX_RECORD_BYTES: usize = NATS_DEFAULT_MAX_PAYLOAD_BYTES - SEAL_HEADROOM_BYTES;

/// The streams that hold the records (ADR#0086 Decision 2).
pub const STREAM_AUDIT: &str = "AUDIT";
pub const STREAM_AUDIT_CHECKPOINT: &str = "AUDIT_CHECKPOINT";
pub const STREAM_DUPLICATE_WINDOW: Duration = Duration::from_secs(120);

/// A lost ack is retried inside the stream's duplicate window (two minutes), so
/// three attempts of a couple of seconds stay well within it.
pub const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_MAX_PUBLISH_ATTEMPTS: u32 = 3;

pub const DEFAULT_BEST_EFFORT_CAPACITY: usize = 1024;
pub const DEFAULT_RETRY_BUFFER_CAPACITY: usize = 256;
pub const DEFAULT_DRAIN_INTERVAL: Duration = Duration::from_secs(1);

/// A checkpoint is signed once this many records have been acknowledged since
/// the last one, so a crash leaves at most this many records past the last
/// signed head, where a deleted tail goes unnoticed.
pub const DEFAULT_CHECKPOINT_EVERY_RECORDS: u64 = 1_000;
/// The longest a chain goes without a checkpoint. An idle chain is signed again
/// at this pace with its head unchanged, as a heartbeat that shows the writer is
/// alive and its chain has not been cut.
pub const DEFAULT_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(60);
pub const DEFAULT_CHECKPOINT_SIGN_TIMEOUT: NonZeroDuration =
    NonZeroDuration::from_secs_const(DEFAULT_ACK_TIMEOUT.as_secs());
pub const DEFAULT_CHECKPOINT_PUBLISH_TIMEOUT: NonZeroDuration =
    NonZeroDuration::from_secs_const(DEFAULT_ACK_TIMEOUT.as_secs() * DEFAULT_MAX_PUBLISH_ATTEMPTS as u64);

/// The JWS `typ` of a checkpoint signature. It keeps a checkpoint signature from
/// being taken as any other JWS signed with the same key, and the reverse.
pub const CHECKPOINT_JWS_TYPE: &str = "trogonai-audit-checkpoint+jws";
/// Ed25519, the one algorithm of the ADR#0038 suite.
pub const CHECKPOINT_JWS_ALGORITHM: &str = "EdDSA";

/// A key id is a `did:key`: this prefix, then the multibase base58btc form of
/// the multicodec-tagged public key (ADR#0038 Decision 1).
pub const DID_KEY_PREFIX: &str = "did:key:";
pub const MULTIBASE_BASE58BTC: char = 'z';
/// The multicodec `ed25519-pub` as its unsigned varint.
pub const ED25519_MULTICODEC: [u8; 2] = [0xed, 0x01];

/// Target of the structured local log lines the writer emits when it refuses an
/// action or drops a record.
pub const LOG_TARGET: &str = "trogon_audit";
