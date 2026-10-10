---
number: "0068"
slug: reference-fields-say-they-are-references
status: draft
date: 2026-10-10
---

# ADR#0068: A Reference Field Says It Is a Reference in Both Its Name and Its Type

## Context

Fields across `proto/trogonai` that point at something stored or owned
elsewhere are spelled inconsistently. `ArtifactRef artifact_ref` stutters the
word twice; `SkillVersionRef version_ref` and `ModelVersionRef version_ref`
carry the referenced kind in the type and a generic `version_ref` in the
field, which reads correctly only once the type is already known.
Out-of-line stored bytes are worse: `Checkpoint.capture_attestation_ref` and
`ExecutionAttemptStarted.host_artifact_ref` are bare `string` fields whose
digest lives in a separate sibling field, so nothing in the schema ties the
locator to what the referenced bytes must hash to. `StoredArtifact.storage_ref` and `Cost.rate_ref` are the
same problem on a single field: an opaque string standing in for a location
or an identifier a reader cannot otherwise recognize as one.

A JSON consumer of these events frequently has no access to the `.proto`
schema, only the wire JSON. Reading `"result": {...}` on its own does not
say whether `result` is the payload itself or a pointer to a payload stored
elsewhere; the field has to say so by name, because the type is invisible at
that layer. Naming a reference by role alone works for a hierarchy position
such as `parent`, because a parent is never mistaken for inline content, but
not for a kinship field such as `ProjectId project_id`, where a bare
`project` would read as an embedded project. A
field that could plausibly hold the thing itself, such as a result, a
response, an attestation, or an attempt's artifact, does carry that risk, and
needs the suffix.

`proto/trogonai/session` and `proto/trogonai/workflows` are excluded from
Rust codegen in `buf.gen.yaml`, so no generated code or compiled caller exists
yet for any of these fields. This is the cheapest the rename and the type
changes will ever be.

## Decision

A reference field's name ends in `_ref` (a structured reference to something
else, carrying more than an opaque string) or `_id` (a single opaque
identifier) exactly when the thing it names could otherwise be mistaken for
inline content by a reader with no schema. The type backs up the name: a
reference is a value object (`ClaimCheck`, `StorageLocator`, a `*Ref`
message, a single-field `*Id` message), never a bare `string`.

- `ArtifactRef artifact_ref`, `ArtifactRef result_ref`, `ArtifactRef
  before_ref`/`after_ref`, `ArtifactRef response_ref`: kept as is. The
  stutter is the name correctly declaring, independent of the type, that the
  field is a pointer to an artifact stored out of line.
- `SkillVersionRef version_ref`, `ModelVersionRef version_ref`: kept as is,
  for the same reason.
- Out-of-line stored bytes referenced by a bare `string` plus a separate
  digest field become a single `ClaimCheck` (a `StorageLocator` plus a
  `Digest`), keeping the `_ref` name: `Checkpoint.capture_attestation_ref`,
  `ExecutionAttemptReady.ready_attestation_ref`,
  `ExecutionAttemptStarted.host_artifact_ref`. The separate digest field is
  retired; the name does not move, because it already said "reference."
- An opaque location string becomes a `StorageLocator` value object, never a
  bare `string`: `StoredArtifact.storage_ref`.
- An opaque identifier string becomes a single-field id message and takes the
  `_id` suffix that names it as such: `Cost.rate_ref` becomes `Cost.rate_id`
  (`PriceRateId`), since nothing about a price rate needs the richer
  `ClaimCheck` shape, only a stable opaque identifier.
- `CheckoutRef` fields named `checkout` (`CreateSession`, `ForkSession`,
  `RecoverSession`, `CreateChildSession`, `SessionStarted`, the workflow
  `Run`) are renamed to `checkout_ref`, for the same reason: a schema-less
  reader seeing `"checkout": {...}` cannot tell a bound reference from an
  inline checkout record.
- Changing a field's type allocates a new field number and reserves the old
  one, so old bytes never decode as the new type. The old name is reserved
  too, unless the field keeps it at the new number. Reservations follow the
  existing style, for example `reserved 7; reserved resume_cursor;` in
  `execution_attempt_started.proto`.

## Consequences

- A bare `string` field naming a location, an identifier, or stored bytes
  elsewhere in `proto/trogonai/session` after this ADR is a bug, not an
  accepted shorthand.
- `buf breaking` against `main` reports the field removals, additions, and
  reserved ranges in every touched file. This is expected and accepted while
  `proto/trogonai/session`, `proto/trogonai/agents`, and
  `proto/trogonai/workflows` remain excluded from codegen; no compiled
  caller observes the break.
- A reviewer reading a field name alone, with no type or schema in hand, can
  tell a reference from inline content: `_ref` or `_id` means elsewhere,
  anything else means here. `parent` and other bare hierarchy-position
  fields are the deliberate exception this ADR does not touch: see [Hierarchy position is referenced by a bare parent
  field](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6310044131/README.md),
  which this ADR narrows rather than contradicts, since a hierarchy position
  is never itself the payload a reader might confuse it for.
- Research and comparison documents and test fixtures that quote these proto
  names are updated to match; documents describing another product's own
  field keep that product's real name unchanged.

## Alternatives Considered

### Name every field by role alone, reference or not

Rejected. This reads cleanly once the schema is in hand, which is the case
inside this repository's own Rust code, but a JSON consumer without the
`.proto` on hand sees only `"result": {...}` and cannot tell a pointer from a
payload. The suffix carries exactly the information the type cannot convey
at that layer.

### Suffix only the primitive strings, leave structured `*Ref` fields bare

Rejected. This still leaves the type doing all the work: a reader has to
already know that `ArtifactRef` is a reference type to read
`artifact: ArtifactRef` as a pointer rather than an embedded artifact. The
name should not depend on the reader already knowing the type's shape.

## References

- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ADR#0035: Session Store Decider Aggregate](./0035-session-store-decider-aggregate.md)
- [ADR#0067: A Session Binds to a Checkout, Not a Workspace](./0067-session-checkout-not-workspace.md)
- [Hierarchy position is referenced by a bare parent field](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6310044131/README.md)
