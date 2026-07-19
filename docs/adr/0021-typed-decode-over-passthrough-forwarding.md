---
number: "0021"
slug: typed-decode-over-passthrough-forwarding
status: accepted
date: 2026-07-07
---

# ADR#0021: Typed Decode over Passthrough Forwarding

## Context

The [ACP](../glossary/acp)-over-[NATS](../glossary/nats) [bridge](../glossary/bridge) decodes every message into typed SDK structs and
re-serializes them (`acp-nats/src/wire.rs`). During the schema 0.11.4 to 1.4.0
catch-up this bit us: fields the pinned SDK did not model were silently
stripped in transit, and unknown `session/update` variants failed decode and
were dropped. The catch-up effort (issue #474) asked whether the bridge should instead forward
payloads losslessly (raw `serde_json::Value` passthrough, typed validation
only where the bridge reads fields), so future spec additions degrade to
"forwarded" instead of "dropped".

## Decision

Keep typed decode. The evaluation concluded that passthrough trades a managed
maintenance cost for the loss of properties the bridge depends on:

1. **Validation at the boundary.** The bridge rejects malformed payloads with
   `InvalidParams` before they reach runners or [JetStream](../glossary/jetstream) durable streams.
   With passthrough, malformed frames propagate and fail deep inside
   consumers, where the blast radius includes persisted garbage in the
   COMMANDS stream.
2. **The bridge reads most of what it routes.** Session ids, cwd, mcp server
   counts, prompt payloads for telemetry spans, response session ids for
   session-ready scheduling: the majority of routed messages are already
   inspected, so "validate only where we read" converges back to typed decode
   for most of the surface anyway.
3. **The failure mode is now managed, not silent.** The original harm was
   silent drift. That is addressed by the tracking foundation instead of by
   loosening the wire layer: decode failures emit a `session_update` /
   `decode_failure` error metric, the weekly freshness workflow files an issue
   the moment upstream moves, and the upgrade ritual in
   `docs/architecture/acp-conformance.md` makes round-trip tests for new
   fields a mandatory part of every bump.
4. **Schema-level leniency reduces the sharp edges.** Since schema 1.x,
   unknown optional fields tolerate errors (`DefaultOnError` annotations), and
   enum extension guidance is landing upstream for v2. The cost of typed
   decode shrinks with each upstream release rather than growing.

## Consequences

- A peer sending a field newer than our pin still loses that field until the
  next bump. The freshness workflow bounds that window to roughly a week of
  detection latency plus the bump turnaround, and the conformance matrix makes
  the gap visible instead of silent.
- Re-evaluate if either condition changes: upstream begins shipping breaking
  schema changes faster than the bump cadence can absorb, or the bridge stops
  reading payloads it routes (for example a pure relay deployment mode). In
  that case a passthrough mode scoped to specific methods is the fallback
  design.
