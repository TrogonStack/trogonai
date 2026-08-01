# Media/Object Storage for trogonai

Decides where the media service from [File and Media
Pipeline](./file-media-pipeline.md) should store bytes: NATS JetStream
Object Store, S3/MinIO, or the Blossom pattern Buzz uses. Produced
2026-07-30, web research against primary docs; unverified items flagged
inline.

## Verdict

**S3/MinIO for both inbound channel media and outbound agent artifacts.
Skip Blossom. Reserve NATS Object Store, if used at all, for small
internal artifacts coupled to stream state.** On the wire, represent
objects as a custom `media://` URI in ACP `resource_link`, resolved by a
trogonai media service that mints fresh presigned URLs at resolution time.

## 1. NATS JetStream Object Store

- Chunked-blob abstraction over a JetStream stream (`OBJ_<bucket>`);
  default chunk 128KB; SHA-256 digests native (ADR-20, nats-architecture-
  and-design). Replication via the underlying stream's `num_replicas`;
  TTL via `max_age`.
- `max_payload` can be set to 64MB but docs recommend staying at or under
  8MB; 1MB is the default (beta-docs.nats.io/reference/config/max_payload).
- Rust: async-nats ships a feature-gated `jetstream::object_store` client
  (Put/Get/Delete/List/Info/Watch/Seal/AddLink). Introduction version and
  parity vs Go/JS unverified.
- Production friction on record: list performance degraded with stored
  volume (nats-server#4680, fixed in #4712); practitioner guidance puts
  the practical per-object ceiling around 5GB and throughput at hundreds
  of MB/s (unverified figures). Suited to build artifacts and app state
  co-located with NATS replication/auth, not arbitrary media.

## 2. S3/MinIO

- Presigned URLs: default and max expiry 7 days (604800s).
- Lifecycle rules give TTL cleanup (`mc ilm rule add --expire-days`);
  enforcement is a background scanner, so TTL is eventual.
- Multipart: 5 MiB to 5 TiB per part, 10,000-part cap (S3 API parity).
- Operational reality: production MinIO is a multi-node erasure-coded
  system (2-16 drive sets, 4:4 default parity), its own failure domain,
  not a sidecar. Single-node has no erasure protection.
- Rust SDKs: `aws-sdk-s3` (1.x, AWS-maintained, works against MinIO) is
  the mature choice; minio-rs is pre-1.0.

## 3. Blossom: skip it

BUD-01 is content-addressing (`GET /<sha256>`), BUD-02 upload, but the
auth (BUD-11) is a signed Nostr kind-24242 event in the Authorization
header. Buzz adopts Blossom because Buzz IS a Nostr platform: every actor
already has a keypair and every event is signed. trogonai has no Nostr
identity layer; adopting Blossom means building a parallel keypair system
just to satisfy its auth format. Its one portable idea, sha256
content-addressing, comes free with NATS Object Store digests or S3 keys.
Its target problem (federated hosting across untrusting third-party
servers) is not trogonai's problem: net-added-complexity trap.

## 4. Wire representation and guardrails

- **`media://` custom scheme in `resource_link.uri`** (spec mandates no
  scheme), resolved by the media service which mints a fresh presigned URL
  at resolution time. Rejected alternatives: raw presigned https URLs
  (time-bombed inside persisted conversation history, leak bucket
  topology) and workspace-relative paths (fail for cross-agent/service
  references).
- **Inline base64 only under ~1MB** including the ~33% base64 and
  JSON-RPC envelope overhead; everything larger, and all channel media
  and artifacts by default, is object-store-first with resource_link as
  the only wire representation. Keeps ACP-over-NATS messages far from the
  max_payload ceiling.
- **TTL policy**: inbound channel media short (days, conversation-scoped
  lifecycle rules); outbound artifacts longer (weeks, deletion tied to
  workspace/session lifecycle since artifacts are often the deliverable).
  This closes the cleanup gap both OpenClaw and Hermes lack.

## Key claims

- Object Store chunk size 128KB; SHA-256 native; TTL via max_age (ADR-20).
- max_payload ceiling 64MB, recommended max 8MB, default 1MB (NATS config
  reference).
- MinIO presigned URLs cap at 7 days; lifecycle TTL is eventual
  (scanner-enforced).
- Blossom auth requires Nostr signatures (BUD-11); Buzz's adoption rides
  its native Nostr substrate, not a standalone pattern.
- Object Store list-at-scale had real, since-fixed production friction
  (nats-server#4680).
- ACP `resource_link.uri` has no mandated scheme; `media://` is a free
  design choice (agentclientprotocol.com/protocol/content).

## Sources

NATS: ADR-20 (nats-architecture-and-design), docs.nats.io obj_store and
walkthrough, beta-docs.nats.io max_payload reference, natsbyexample.com,
docs.rs/async-nats object_store, nats-server#4680. MinIO: docs.min.io
(mc-share-download, lifecycle management, erasure coding), minio-limits.md,
distributed README, crates.io/aws-sdk-s3, github.com/minio/minio-rs.
Blossom: BUD-01/02/11 specs (github.com/hzrd149/blossom),
github.com/block/buzz (ARCHITECTURE.md, README.md). ACP:
agentclientprotocol.com/protocol/content, docs.rs/agent-client-protocol
ContentBlock. Practitioner posts (flagged unverified where used):
timderzhavets.com, sethitow.com.
