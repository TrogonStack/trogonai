---
number: "0064"
slug: schema-constraints-as-documentation
status: draft
date: 2026-09-11
---

# ADR#0064: Invariants Are Declared in the Schema Before Anything Enforces Them

## Context

The agents, session, and usage contracts state a large number of invariants,
and they state almost all of them in prose. Binding names are "nonempty,
case-sensitive, and unique across every collection below". A revision number
is "positive". A digest algorithm is "only `sha256` with a 32-byte value".
An enum's "unspecified and unknown values are invalid". A oneof requires
"exactly one known arm".

Prose of this kind is a claim that nothing checks. It is not merely
unenforced at runtime, which would be a normal staging decision; it is
unverified as a statement. Nothing confirms that the sentence is even
coherent against the field it describes, so a comment can outlive the field
it constrains, contradict a sibling comment, or describe a bound that the
type cannot hold. The review round on the provider credential research
surfaced this as one defect class among several that share a root: a claim
written in a comment has no mechanical relationship to the thing it claims.
Stale counts and ADR citations that resolved to nothing were the same failure
in a different costume, and both of those are now checked by CI.

The obvious remedy is to make the invariants executable: annotate them and
generate validators. Two facts about this repository make that the wrong
first move.

[ADR#0009](./0009-protocol-buffers-wire-contracts.md) establishes Protocol
Buffers as the contract language, but `buf.gen.yaml` excludes
`proto/trogonai/agents`, `proto/trogonai/session`, and `proto/trogonai/usage`
from code generation, so their shape can churn without dragging the Rust
workspace along. That is 175 of the 207 files in the module, and it covers
every package discussed above. There is no generated Rust type for any of
them, so there is nothing for a generated validator to attach to. Enforcement
is not deferred for these packages; it is not yet addressable.

The exclusion also sets the boundary in the other direction. Both codegen
plugins run with `include_imports: true`, so an import added to a generated
package enters the generated crate with it. Annotating a generated package
therefore pulls `buf/validate/validate.proto` into the Rust build, which is a
compilation change, whereas annotating an excluded package is inert.

The invariants are worth declaring anyway, and declaring them is separable
from enforcing them. `buf lint` compiles and type-checks every CEL expression
in the schema against the descriptor it constrains. An annotated invariant is
therefore checked to be well-formed and type-correct even when nothing
evaluates it against a message, which is strictly more than a sentence in a
comment has ever been.

## Decision

### 1. Invariants stated in prose are also declared as `buf.validate` constraints

`buf.build/bufbuild/protovalidate` is a module dependency. Field rules,
oneof rules, and message-level CEL express the invariants the comments
already state.

### 2. The annotations are documentation, and nothing in this repository enforces them

No codegen plugin emits validators. No Rust crate takes a validation runtime
dependency. Adding the dependency and the annotations changes no generated
output, which `mise run github-actions:assert-proto-generated` proves.

This is the cost of the decision and it is accepted deliberately: an
annotation looks enforced. A reader who sees `min_len: 1` may assume some
layer rejects the empty string, and today no layer does. The alternative was
to leave the invariant in a comment, where it looks unenforced and is also
unverified, and where promoting it later means rediscovering what the comment
meant. Declared constraints are wrong about who enforces them; prose
constraints are wrong about that too, and are additionally unchecked.

### 3. Only packages excluded from code generation are annotated

Annotating a generated package is a compilation change, per the
`include_imports` coupling above. The moment a package is promoted into
`buf.gen.yaml` is therefore the moment to decide whether its annotations
become enforced, and that decision belongs to that promotion rather than
to this one.

### 4. Every annotation restates an invariant the file already states

An annotation is a second expression of an existing claim, never a new claim
in a more compact notation. Where prose is silent, the schema stays silent:
a required free-text field with no stated nonemptiness rule gets no
`min_len`, an enum whose comments do not reject unknown values gets no
`defined_only`, and a bound that prose only implies stays unannotated.
Minting a new constraint is a contract decision, and the one generalization
this pass does make is recorded as decision 5 below rather than applied
quietly.

### 5. An identifier is nonempty, and that is a decision recorded here

Most packages never say in prose that an identifier cannot be the empty
string. Annotating identifiers anyway is the one place this pass asserts
something the comments do not, so it is stated here rather than left to look
like an oversight or a silent house style.

[ADR#0040](./0040-contract-field-vocabulary.md) classifies a string field by
asking whether any mechanism resolves the value: a value nothing looks up is
a display label, and a value that is looked up, joined on, or used as a key
into a catalog is an identifier or a handle. The empty string resolves to
nothing under that test, so an empty identifier is not a degenerate
identifier; it is a value from the other category wearing an identifier's
field name.

`(buf.validate.field).string.min_len = 1` therefore applies to every field
that ADR spells as an identifier or a handle (`id`, `<referent>_id`, bare
`parent` under that ADR's placement exception, `name`, `<referent>_name`) and
to every version string naming an immutable release. It does not apply to
`display_name`, to free-form text, to human-readable non-contractual detail
fields, or to opaque and derived data such as digests and serialized
payloads, all of which that ADR places outside the test's scope.

Two exclusions carve out of that rule, and both are read out of the
annotated files rather than chosen for convenience. A field whose own
comment blesses the empty string is skipped, because there a `min_len`
would contradict a stated behavior instead of restating one:
`ToolCallRequested.operation_id` is "empty for a call that reserves no
operation", `ExecutionAttemptStarted.previous_attempt_id` is "empty exactly
when attempt_number is 1", and seven more in the session plane read the same
way, among them `ToolCallRequested.parent_tool_use_id`, which carries the
same top-level linkage that `ToolUseBlock` documents as empty. A comment
that says "empty" about some other value does not trigger the exclusion,
which is why `ProviderToolIntentRejected.rejection_id` keeps its constraint:
the emptiness it describes belongs to the provider's identifier, not to the
runtime-minted one it is there to replace.

The second exclusion is that ADR's wire-fidelity exemption, which governs
`ToolUseBlock` and `ToolResultBlock`. Their job is to reproduce what a
provider actually sent, so the platform's identifier vocabulary does not
describe their value space and has no standing to narrow it.

This is the only constraint in this pass that generalizes. Everything else
restates a sentence from the file it annotates, per decision 4 above.

### 6. Admission-time rules stay in prose

A constraint over a single message is expressible. A rule that needs live
context is not, and no annotation pretends otherwise. "Must resolve to at
least one authorized dependency before Session start", "multiple matching
connections fail admission", and digest agreement between separately
transmitted values all depend on state outside the message, and remain
prose addressed to the admission path.

## Consequences

The invariants become greppable and type-checked, and a contradiction
between a constraint and its field is now a lint failure rather than a
comment nobody reread.

Enforcement stays a single, separable step. The annotations are the input a
validator generator needs, so enabling enforcement for a package means
promoting it into codegen and adding a plugin, not first recovering
invariants from prose.

Until that step, the schema declares rules that no layer applies, and
[ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md)'s
admission path remains the only thing that actually rejects a malformed
declaration.
