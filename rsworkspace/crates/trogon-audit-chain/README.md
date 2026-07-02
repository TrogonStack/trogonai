# trogon-audit-chain

Tamper-evident SHA-256 hash chain for audit events durably stored on NATS
JetStream.

## What this is

A linear hash chain, in the sense of a blockchain's block-to-block linkage
without the block or the chain: each audit event's hash commits to its own
canonical bytes and to the previous event's hash, so verifying the chain
means recomputing every link once and checking it against what was stored.

The crate has two layers:

- A pure chain core (`ChainHash`, `CanonicalEventBytes`, `extend`,
  `verify_chain`) with no NATS dependency. This is the part that needs to be
  correct; it is exhaustively unit tested including tamper cases.
- A JetStream integration layer (`ChainPublisher`, `verify_stream_chain`)
  built on `trogon-nats`'s `JetStreamPublisher` / `JetStreamGetStreamInfo` /
  `JetStreamGetRawMessage` traits, tested entirely against `trogon-nats`'s
  `test-support` mocks (no real NATS server required, per ADR 0010).

## Design: chain links as message headers, not a companion KV

The chain hash and previous-hash for each entry are carried as headers on the
JetStream message itself (`Trogon-Chain-Hash`, `Trogon-Chain-Previous-Hash`),
following the `Trogon-`-prefixed application header convention established by
ADR 0013. This was chosen over a companion KV bucket keyed by stream sequence
for three reasons:

1. **Colocation.** The event and its chain metadata are one atomic unit at
   publish time and one atomic unit at read time. A companion KV requires two
   writes (stream publish, then KV put) that can fail independently, and two
   reads to reconstruct one chain entry, with no built-in way to make the
   pair atomic.
2. **Precedent.** ADR 0013 already established message headers as the place
   for provenance metadata (`Trogon-Origin-Stream-Sequence`), and
   `trogon-decider-nats` follows the same convention for event metadata
   (`Trogon-Event-Type`, `Trogon-Header-{name}`). Headers keep this crate
   consistent with how the rest of the codebase attaches metadata to
   JetStream messages.
3. **Replay is a single stream scan.** `verify_stream_chain` reads the stream
   once, sequence by sequence, and has everything it needs (payload,
   previous hash, hash) from that one message. A KV design would need a
   second data source kept in sync with the stream's retention and replay
   semantics (what happens to KV entries when a stream message is purged or
   the stream's retention policy trims old entries?).

The tradeoff: headers add a small amount of bytes to every message and are
visible to anything that reads the stream, whereas a KV bucket could in
principle apply different access controls. That tradeoff was judged
acceptable because chain hashes are not secret; their entire purpose is to be
checked by verifiers.

## Threat model

This is **tamper-evident, not tamper-proof**. The chain detects
after-the-fact modification of events it already covers; it does not prevent
modification, and it does not prevent an actor with sufficient JetStream
privileges from defeating detection entirely.

What `verify_chain` / `verify_stream_chain` catch:

- Editing an event's payload in place without recomputing its hash
  (`ChainBreak::HashMismatch`).
- Reordering, inserting, or dropping entries so the recorded sequence numbers
  no longer run contiguously from 1 (`ChainBreak::SequenceGap` /
  `ChainBreak::DoesNotStartAtOne`).
- Forging an entry with a self-consistent hash (`hash == extend(previous,
  event)`) that does not link back to the real previous entry's hash
  (`ChainBreak::PreviousHashMismatch`).
- Deleting a message inside the verified sequence range on a JetStream
  stream (`ReplayVerifyError::MissingMessage`): a missing message is treated
  as a break, not silently skipped, specifically so a delete cannot be used
  to erase evidence undetected.

What it does **not** catch:

- **A rewinder with stream purge/delete and republish rights.** An actor who
  can delete every message in the stream from some point forward and
  republish a new, internally self-consistent chain from that point can
  produce a chain that verifies cleanly; `verify_chain` only checks internal
  consistency, it has no external reference for what the chain's tip
  *should* be. This is the chain's central residual risk.
- **Tip forgery on a fresh reader.** Anyone who can read the stream can
  recompute the whole chain and get a hash, but if that recomputation is the
  only source of truth, a rewritten history recomputes to a hash that looks
  equally valid. Detecting this requires an independent, out-of-band record
  of what a known-good tip hash was at some point in time.
- **Confidentiality.** Chain hashes are visible to any stream reader; nothing
  here encrypts event payloads.

**Closing the rewind gap (future work, not implemented here):** periodically
publish the current chain tip hash to a destination the audit chain's own
administrators cannot rewrite: an external append-only log, a timestamping
service, a separate organization's system, or a public transparency log.
Anyone verifying the chain later checks not just internal consistency but
also that the tip at time T matches what was anchored externally at time T.
`ChainPublisher::current_tip` already exposes the value that would be
anchored; this crate does not implement the anchoring itself.

## Relationship to AUDIT-COMPLIANCE-1.0

This crate implements the linear hash-chain construction from
AUDIT-COMPLIANCE-1.0 Section 9.2 (each entry's hash is `SHA-256(previous_hash
|| canonical_event_bytes)`) and the fold-based chain verification described in
Section 9.7, adapted to run over a JetStream stream instead of an in-memory
log. Event canonicalization follows the sorted-key, no-whitespace JSON
approach described in Section 4.4.

The concept and the reference commitment-engine behavior are adapted from the
[Agent Governance Toolkit](https://github.com/microsoft/agent-governance-toolkit)'s
`AUDIT-COMPLIANCE-1.0` specification and its MIT-licensed in-memory commitment
engine reference implementation. Nothing here is a copy of that code; the
Rust types, error handling, and JetStream integration are new, built to this
repository's own conventions.

**This crate does not claim full Level 3 conformance.** Explicitly out of
scope:

- The Merkle tree and inclusion-proof machinery of Sections 9.3-9.6. This
  crate implements the linear chain only; there is no batching into Merkle
  trees and no compact inclusion proofs for individual entries.
- The Compliance Framework Engine, Decision BOM, and Semantic Delta Engine.
- A full Commitment Engine beyond hash-chaining (e.g. external timestamping,
  witness co-signing).
- A REST API.

What this crate delivers is a correct, well-tested core: a durable,
replicated hash chain over JetStream that is strictly stronger than AGT's
shipped commitment engine on the one dimension that engine is weakest on
(AGT's reference engine is in-memory only and loses its chain on restart;
this one is backed by a JetStream stream and can be replayed and re-verified
from scratch at any time).

## Usage sketch

```rust,ignore
use trogon_audit_chain::{CanonicalEventBytes, ChainPublisher, verify_stream_chain};

// Publishing:
let publisher = ChainPublisher::new(jetstream_context);
let event_bytes = CanonicalEventBytes::from_json_value(&event)?;
publisher.publish("audit.chain", async_nats::HeaderMap::new(), event_bytes).await?;

// Verifying (e.g. periodically, or before trusting the chain after a restart):
verify_stream_chain(&jetstream_context).await?;
```

## Testing

The pure chain core is tested without any NATS dependency at all. The
JetStream integration is tested against `trogon-nats`'s `test-support` mocks
(`MockJetStreamPublisher`, `MockJetStreamPublishMessage`); no real NATS
server or testcontainer is used, consistent with ADR 0010.
