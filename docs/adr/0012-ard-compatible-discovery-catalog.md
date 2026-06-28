---
number: "0012"
slug: ard-compatible-discovery-catalog
status: accepted
date: 2026-06-28
---

# ADR 0012: ARD-Compatible Discovery Catalog

## Context

The Agent Resource Discovery specification defines an HTTP and JSON discovery
surface for cataloging agentic resources such as A2A agent cards, MCP server
cards, catalogs, registries, and other callable resources. The current upstream
snapshot used by this repository is `ards-project/ard-spec`
`832347bda6af4ce3b61bd250c14a8e899d3ff942`.

The current ARD snapshot uses:

- `ai-catalog.json` manifests with `specVersion: "1.0"`
- logical identifiers shaped as `urn:air:<publisher>:<namespace>:<name>`
- catalog entries with `identifier`, `displayName`, `type`, and exactly one of
  `url` or `data`
- a registry HTTP surface where `POST /search` is mandatory and `GET /agents`
  and `POST /explore` are optional

Trogon already uses NATS as its internal backbone under
[ADR 0003](./0003-ai-protocol-transport-taxonomy.md). ACP, MCP, and A2A remain
the execution protocols reached through their own protocol and transport
packages under [ADR 0004](./0004-protocol-and-transport-layering.md).

The design question is whether ARD should become part of the internal routing
model, or whether it should be treated as an external discovery compatibility
surface over the existing platform.

## Decision

Implement ARD as an external discovery and catalog compatibility boundary.

ARD describes what resources exist and where their protocol-owned descriptors can
be found. It does not execute work, route protocol messages, authorize runtime
calls, or replace ACP, MCP, A2A, or the NATS backbone.

NATS remains the internal backbone for first-party routing, events, queueing,
durable catalog updates, indexing work, and storage adapters. ARD HTTP and JSON
are accepted as an external compatibility exception under
[ADR 0003](./0003-ai-protocol-transport-taxonomy.md) and
[ADR 0009](./0009-protocol-buffers-wire-contracts.md) because ARD itself defines
the public interoperability surface.

Use this package shape as the implementation grows:

- `ard-catalog` owns ARD-compatible value objects, wire types, validation,
  manifest import/export, and conformance-facing behavior. It has no NATS
  dependency.
- `ard-registry` owns the future ARD HTTP registry runtime only once a live
  registry API is implemented.
- `ard-nats` owns future NATS, JetStream, and KV adapters only once catalog
  persistence or eventing is implemented.

The first implementation slice should prove the catalog model and static
manifest contract before adding registry search, federation, or trust policy
enforcement.

## Invariants

- ARD is discovery-only. It returns descriptors and registry metadata; it does
  not invoke tools, sessions, tasks, prompts, or agent work.
- ACP, MCP, and A2A execution remains in their existing protocol packages.
- ARD identifiers are logical identifiers, not transport routing keys.
- Raw `urn:air:*` values never appear directly in NATS subject segments.
- NATS subject and KV keys use a derived NATS-safe value such as an
  `ArdStorageKey`.
- ARD wire or input types are converted into validated domain values once at the
  boundary.
- Domain values must remain valid after construction.
- `url` and `data` are mutually exclusive in both domain values and emitted ARD
  JSON.
- ARD media type strings round-trip unless a boundary explicitly rejects them.
- `trustManifest` metadata is preserved but does not imply runtime
  authentication or authorization.
- First-party internal catalog events and schemaless persistence records prefer
  Protocol Buffers unless the stored value is intentionally the ARD JSON
  artifact.

## Design Rules

Keep ARD-specific behavior isolated. The ARD specification has already changed
identifier naming from older `urn:ai:*` notes to the current `urn:air:*` shape,
so prefix parsing, schema validation, conformance assumptions, media type
handling, and registry response shaping belong in ARD-owned packages.

Do not reuse A2A-specific identifiers or agent card types as the generic catalog
domain. A2A agent cards can be exported as ARD catalog entries with media type
`application/a2a-agent-card+json`, but the neutral catalog model should not be
A2A-shaped.

Do not expose NATS subjects, stream names, KV bucket names, or derived storage
keys through the public ARD API. The registry edge reconstructs ARD-compatible
JSON from validated catalog data.

Keep search ranking behind an application boundary. `representativeQueries`
can feed lexical or vector indexing, but the `score` returned by ARD search is
relevance, not trust or authorization.

## Consequences

Trogon gains compatibility with an emerging discovery ecosystem without pushing a
draft discovery spec into core runtime semantics.

The REST/OpenAPI surface is justified because ARD requires that public
interoperability shape. Internal first-party communication continues to prefer
NATS, ConnectRPC where a first-party service API is necessary, and Protocol
Buffers for owned wire contracts.

Future ARD spec drift should be absorbed by ARD-owned value objects and wire
types instead of spreading conditional parsing across ACP, MCP, A2A, or NATS
packages.

## References

- [ARD-Compatible Discovery Catalog Plan](../../PLAN.md)
- [ADR 0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR 0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR 0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR 0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
