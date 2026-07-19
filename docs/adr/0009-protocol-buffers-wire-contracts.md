---
number: "0009"
slug: protocol-buffers-wire-contracts
status: accepted
date: 2026-06-08
---

# ADR#0009: Protocol Buffers Wire Contracts

## Context

The repository already uses Protocol Buffers for generated [protocol](../glossary/protocol) packages and
scheduler message contracts. [ADR#0003](./0003-ai-protocol-transport-taxonomy.md) prefers [NATS](../glossary/nats)-backed internal boundaries
when both sides are first-party runtime components, and prefers [ConnectRPC](../glossary/connectrpc) over
direct gRPC after a first-party service API surface is necessary. [ADR#0005](./0005-polyglot-workspace-layout.md) names
Protocol Buffers as the preferred cross-language contract, with NATS-backed
messages for internal paths and ConnectRPC for service APIs.

Those decisions cover API style and workspace boundaries, but they do not state a
general rule for first-party machine-to-machine wire and persistence contracts.
Without one rule, new services, event payloads, queue messages, schemaless
storage values, and cross-language packages can choose JSON, MessagePack, custom
binary encodings, or ad hoc structs for reasons that are local to one package
instead of stable across the system.

The repository needs wire contracts that are typed, evolvable, code-generated,
and usable across Rust, TypeScript, Elixir, Go, Python, and future language
workspaces.

## Decision

Prefer Protocol Buffers for first-party machine-to-machine wire and schemaless
persistence contracts when the contract benefits from schema evolution, generated
clients, or cross-language reuse.

Use Protocol Buffers by default for:

- First-party service API schemas, normally served through ConnectRPC after ADR
  0003 selects an API surface.
- Cross-language request, response, [command](../glossary/command), and [event](../glossary/event) contracts.
- Durable queue, [stream](../glossary/stream), and event payloads owned by this repository.
- Schemaless persistence values, including KV records, document-store records,
  state snapshots, and blob metadata, when this repository owns the stored value
  shape.
- Internal backbone messages whose payload is not already fixed by an external
  protocol.
- Generated SDK, client, or protocol packages shared across language workspaces.

NATS-backed internal messages can use the same Protocol Buffers schema governance
without becoming ConnectRPC or gRPC services. Choose ConnectRPC or direct gRPC for
the API surface only after [ADR#0003](./0003-ai-protocol-transport-taxonomy.md) rules out the internal backbone as the right
boundary.

Schemaless storage does not remove the need for a schema. When a KV store,
document store, blob metadata record, or similar persistence layer stores
first-party structured data without enforcing columns or constraints, the stored
value should still be encoded from a versioned Protocol Buffers contract.
Storage keys, buckets, collections, or subjects can route records, but they are
not a substitute for a typed value schema.

Store source schemas under `proto/` using stable package names and versioned
namespaces. Generated code belongs in the language workspace that consumes it or
in a dedicated generated protocol package when multiple packages need the same
contract.

Prefer the protobuf binary encoding on internal machine-to-machine paths. Use
protobuf JSON only when a human-readable, browser-facing, diagnostic, or
interoperability boundary requires JSON while preserving the same protobuf
schema.

## Design Rules

Treat `.proto` files as the source of truth for the wire contract. Do not define
parallel hand-written request, response, event, or state shapes unless they are
domain value objects that convert at the boundary.

Use Protocol Buffers' compatibility model deliberately:

- Add fields with new field numbers.
- Reserve removed field numbers and names.
- Avoid changing field meaning after release.
- Prefer explicit messages over untyped maps for domain concepts.
- Use well-known types for timestamps, durations, and structured values when they
  match the domain.
- For schemaless persistence, keep enough type or version information at the key,
  envelope, or message boundary to select the correct decoder during migrations
  and repairs.
- Keep [transport](../glossary/transport) concerns such as NATS subjects, HTTP paths, headers, and retry
  policy outside the domain message unless they are part of the stable contract.

Generated protobuf types should not leak primitive obsession into domain code.
Convert into richer domain types at package boundaries when validation, units,
identity, or invariants matter.

## Exceptions

Do not use Protocol Buffers when the boundary is owned by a protocol, platform, or
ecosystem that requires another format.

Valid exceptions include:

- [MCP](../glossary/mcp), [ACP](../glossary/acp), [JSON-RPC](../glossary/json-rpc), webhook, or third-party API surfaces with protocol-defined
  JSON contracts.
- Human-edited configuration files, which follow [ADR#0007](./0007-configuration-sources.md).
- OpenAPI or REST-like HTTP contracts allowed by [ADR#0003](./0003-ai-protocol-transport-taxonomy.md).
- Logs, metrics, traces, and telemetry export paths governed by [ADR#0008](./0008-opentelemetry-observability.md) or by a
  deployment system.
- Plain text, Markdown, HTML, or binary file content where the payload format is
  the product data rather than the envelope contract.
- Short-lived experiments under `experiments/`, until promoted to an ADR-governed
  package or service boundary.

When an exception is used for a first-party machine-to-machine contract, document
the reason near the boundary that owns the format.

## Consequences

- New first-party wire and schemaless persistence contracts have one default
  schema language.
- Cross-language clients and services can share generated packages instead of
  duplicating ad hoc structs.
- Reviewers can ask for an explicit exception when a new internal contract uses
  JSON or another encoding.
- Protocol Buffers schema governance becomes part of API and event design, not an
  implementation detail.
- Existing JSON or custom-encoded internal contracts can migrate when touched for
  related work.

## References

- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0005: Polyglot Workspace Layout](./0005-polyglot-workspace-layout.md)
- [ADR#0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR#0008: OpenTelemetry Observability](./0008-opentelemetry-observability.md)
