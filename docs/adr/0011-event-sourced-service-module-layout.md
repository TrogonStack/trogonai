---
number: "0011"
slug: event-sourced-service-module-layout
status: accepted
date: 2026-07-08
---

# ADR 0011: Aggregate-Oriented Module Layout for Event-Sourced Services

## Context

Several first-party services implement an event-sourced model: typed commands
decide events, an evolve function folds events into state, snapshots compact that
state, and read-side processors project the stream into runtime effects.
`trogon-scheduler` and `trogon-gateway` both follow this shape.

ADR 0002 governs package boundaries but deliberately stops short of prescribing
intra-package module structure. `rsworkspace/crates/AGENTS.md` governs value
objects, boundary types, and errors, but describes a flat crate-root layout that
fits value-object and library crates, not an event-sourced service.

With no rule, two failure modes appeared. First, code was organized by layer at
the crate root: a top-level `commands/` module in a crate that hosts more than
one stream reads as "the crate's commands," when the commands actually belong to
one specific stream among several. Second, aggregates were named after the
mechanism rather than the domain: `CredentialLifecycle*` names describe that the
type participates in an event-sourced lifecycle, which every event-sourced
aggregate does by definition, so the qualifier carries no information and
compensates for the fact that the opposing concern (the secret material) was
never named on the same axis.

The repository needs one rule for how event-sourced code is organized and named,
and a named reference implementation.

## Decision

Organize event-sourced code by aggregate, not by layer. Name aggregates as bare
domain nouns. `trogon-scheduler` is the reference implementation.

### Organize by stream

The unit of organization is the stream, the workflow or aggregate it represents,
not a technical layer. There is one module per stream, named for that stream.
`commands`, `state`, `snapshot`, `domain`, and the read-side `processor` are
subdivisions inside a stream, never top-level buckets that imply the whole crate
is a single command model.

A stream module contains:

- `commands/`: one file per command decider, named after the command. Each file
  holds the command struct and its `Decider` implementation.
- `state`: the aggregate state type, `initial_state`, `evolve`, and the
  decide-time and evolve-time validators.
- `snapshot`: the snapshot policy and the state-to-persistence codec.
- `domain`: the aggregate's value objects and event definitions, one type per
  file.
- `processor` (or a named projection): the read model that consumes the stream.
  A processor's own rebuildable checkpoint store nests under that processor.
- The aggregate's persistence and command handler: the event store, stream and
  subject configuration, and the handler that executes commands against the
  store. These belong to the aggregate and live inside its module, next to the
  commands they serve. They are not hoisted to the crate root.

A crate that owns exactly one aggregate places that aggregate at the crate root,
so a root-level `commands/` reads correctly (`trogon-scheduler`: the crate is the
schedules aggregate). A crate that hosts multiple aggregates or bounded contexts
nests each aggregate under its own noun module (`trogon-gateway`: the credential
aggregate lives under `credential/`, separate from the webhook-ingress sources
and the secret backends).

### Name a stream for its workflow, as a noun

A stream is not always a static entity. More often it captures a workflow, a
process that unfolds over time. Name the stream for that workflow, expressed as a
noun: a workflow made noun. The name states which workflow the stream is, never
that it happens to be event-sourced.

Do not qualify a stream with mechanism words such as `Lifecycle`, `Manager`,
`Service`, or `Handler`. Every event-sourced stream has a lifecycle and is
handled by something, so those words name the machinery, not the workflow, and
carry no information. `Lifecycle` is the clearest offender: it restates the
pattern.

Name opposing concerns on the same axis instead of qualifying one of them. A
credential has two: the workflow that provisions and maintains it over time
(request, activate, rotate, revoke), and its secret material. Name them
`Credential` (the stream, event-sourced, holds no secret bytes) and `Secret`
(the material, held by the secret store). Do not name the first
`CredentialLifecycle` to distinguish it from the second; naming the second
`Secret` already does that.

### Value objects

Domain value objects for an aggregate live under that aggregate's `domain`
module, one file per type, with the construction and error rules from
`rsworkspace/crates/AGENTS.md`. The flat `src/{type}.rs` placement in the crate
conventions applies to value-object and library crates that own no aggregate. The
read model depending on the aggregate's `domain` is expected, not a layering
violation.

### Domain stays free of infrastructure

`commands`, `state`, `snapshot`, `domain`, and `processor` contain domain and
application logic free of transport and persistence SDKs. Convert at the
boundary per ADR 0009. Infrastructure adapters (NATS/JetStream stores, stream and
subject configuration, KV stores) are thin and live inside the aggregate module
that owns them, not scattered at the crate root.

## Design Rules

- One command decider per file. Do not accumulate multiple deciders, the state
  type, the snapshot codec, and conversions in one module.
- Keep the fold (`evolve`) separate from the snapshot format.
- Name a stream for the workflow it represents, expressed as a noun (a workflow
  made noun), not for the event-sourcing mechanism. Reject `Lifecycle`,
  `Manager`, `Service`, `Info`, and similar mechanism or filler qualifiers in
  stream, event, state, and command type names.
- Persisted identifiers follow the aggregate noun: proto message names, stream
  names, subjects, and KV keys. Because a persisted message's package path or
  fully-qualified name is embedded in storage keys (ADR 0009), renaming an
  aggregate is a migration. Do it before the contract ships; treat it as
  storage-breaking afterward.
- Value objects follow `rsworkspace/crates/AGENTS.md`, located under the
  aggregate's `domain`.

## Consequences

- Event-sourced code is grouped by the thing it models, so a reader sees one
  aggregate's full write and read model in one place, and a multi-aggregate crate
  does not imply that one aggregate speaks for the whole crate.
- Aggregate names carry domain meaning instead of restating the pattern. The
  credential aggregate is `Credential`; the secret material is `Secret`.
- The value-object placement contradiction between
  `rsworkspace/crates/AGENTS.md` and real aggregate crates is resolved.
- `rsworkspace/crates/AGENTS.md` gains a pointer to this ADR and to
  `trogon-scheduler` as the reference.
- `trogon-gateway` migrates: the credential aggregate consolidates under
  `credential/`, and `CredentialLifecycle*` names and the
  `...credential.lifecycle...` stream, subject, and proto identifiers drop the
  `lifecycle` qualifier. This is done before the contract ships.

## References

- [ADR 0002: Rust Crate Boundaries](/adr/0002-rust-crate-boundaries)
- [ADR 0009: Protocol Buffers Wire Contracts](/adr/0009-protocol-buffers-wire-contracts)
- `rsworkspace/crates/AGENTS.md`
