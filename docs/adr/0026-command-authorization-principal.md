---
number: "0026"
slug: command-authorization-principal
status: draft
date: 2026-07-15
---

# ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution

## Context

`trogon_decider_runtime::execution::CommandExecution` is the single entry
point every native command execution goes through: it loads history, replays
it, calls `Decider::decide`, encodes the resulting events, and appends them.
Its builder carries an event store, the command, an optional write
precondition, an optional snapshot policy, an event id generator, and
`headers: Headers` -- a freeform, caller-supplied metadata bag. Nothing in
that builder, nothing in the `Decider` trait itself, and nothing in the WASM
mirror (`trogon_decider_wasm_runtime::execution::WasmCommandExecution`, which
has the identical shape) represents who is submitting a command or what they
are allowed to do. `decide` runs against decider state and the command
payload only.

`docs/architecture/event-metadata.md` is explicit that the runtime should not
derive headers generically: an application that wants a fixed header
(tenancy, correlation, and by the same reasoning, caller identity) must build
it itself before calling `CommandExecution::with_headers`. In practice this
means the closest thing to "who acted" today is whatever string an
application chooses to put in a header, which `decide` never sees and which
carries no authorization semantics -- it is envelope metadata for storage, not
an input the runtime checks before evaluating a decision.

Command execution is also reached from more than one direction. The A2A
gateway resolves caller identity at ingress
([ADR#0017](./0017-aauth-agent-authentication.md)), but internal callers such
as `trogon-scheduler`'s worker processor construct `CommandExecution::new`
directly, off the gateway path entirely. Any authorization hook that only
lives at the gateway leaves every non-gateway caller of `CommandExecution`
unenforced.

AAuth itself is not a ready-made carrier for this. Its three-party auth token
(`aa-auth+jwt`) does carry a `principal` claim, but on the wire it is an
`Option<&str>` (`trogon-aauth-person::mint::MintInputs::principal`,
mirrored in `trogon-aauth-as`) -- an optional, unstructured string naming a
directed user, with no claim set describing scopes, roles, or a stable
principal kind. That is sufficient for what the draft needs at the protocol
level; it is not something `decide` can safely branch on directly, and it
gives no way to distinguish an agent principal from a person principal or an
absent claim from an empty one.

## Decision

### 1. A typed `CommandPrincipal`, separate from `Headers`

Introduce a `CommandPrincipal` value type carried on `CommandExecution` and
`WasmCommandExecution` through a new builder method
(`with_principal`), distinct from `Headers`. `Headers` stays what
`event-metadata.md` already defines it as: envelope metadata for the stored
event. `CommandPrincipal` is an authorization-time input evaluated before
`decide` runs; it is not required to be persisted verbatim, and applications
that want an audit trail of who acted still derive and set their own header
from it, the same way they derive any other required header today.
`CommandPrincipal` models a principal kind (agent, person, service),
a stable principal identifier, and an opaque claims/scope set -- structured
enough for an authorizer to make a decision, without prescribing a policy
language.

### 2. An authorizer trait hook that runs before `decide`

Add a `CommandAuthorizer<C: Decider>` trait with one method that takes the
principal and the command and returns either `Ok(())` or a typed denial.
`CommandExecution`/`WasmCommandExecution` gain an optional builder slot for
an authorizer, defaulting to an `AllowAll` no-op so existing callers keep
compiling and behaving exactly as they do today -- the same opt-in shape
`WithoutSnapshots` already uses for the snapshot builder slot. When an
authorizer is configured, `execute()` calls it after the command's stream id
and the loaded state are available (an authorizer may need to know the
target stream) but strictly before `evaluate_decision`/`decide` runs, so a
denied command never reaches domain logic and never spends replay or, on the
WASM path, guest fuel on work that will be rejected.

### 3. Both native and WASM dispatch paths

For the WASM path, the authorizer check sits in
`WasmCommandExecution::execute` right after `call_stream_id` resolves the
target stream, before `create_session`/`replay_events`/`decide` run --
mirroring the native placement and avoiding the cost of instantiating guest
session state for a command that will be denied. The trait itself does not
differ between paths; only where each path can cheaply evaluate it differs,
because the WASM path does not have `state` available in host-native form at
that point (only the stream id and the command envelope).

### 4. Composing with AAuth given its optional-string principal

The gateway (or any other ingress boundary that already runs AAuth
verification) is where a verified identity becomes a `CommandPrincipal`: the
PoP-verified agent's `sub`/`cnf.jwk` thumbprint maps to an agent principal,
and an `aa-auth+jwt`'s `principal` string, when present, is carried as an
opaque hint attached to that principal rather than trusted as a scoped claim
on its own. This ADR does not change AAuth's wire shape or fix the
optional-string limitation -- that is [ADR#0017](./0017-aauth-agent-authentication.md)'s pinned draft shape. The
mapping boundary is the one place that has to absorb the limitation, and it
must fail closed: a missing or unparsable principal where one is required is
a denial, never a silent anonymous principal.

### 5. Missing or invalid principal

With no authorizer configured, behavior is unchanged (today's implicit
"anyone can submit any command"). Once an authorizer is configured, a command
executed with no principal, or a principal the authorizer cannot validate, is
a hard failure -- a new `Unauthorized` variant on `CommandError`/
`WasmCommandError` -- not a fallback to an anonymous or default-trust
principal.

## Alternatives Considered

### Encode the principal as another `Headers` entry

Rejected. `Headers` is a caller-suppliable, un-typed string bag that already
persists into the event envelope; nothing distinguishes a header a verified
ingress layer set from one a client typed by hand, so an authorizer reading
it could not tell a validated identity from a spoofed one. It also conflates
an authorization-time input with the audit-trail output `event-metadata.md`
already defines headers to be.

### Authorize only at the gateway, trust everything past it

Rejected. Not every command execution path goes through the A2A gateway --
`trogon-scheduler`'s worker processor calls `CommandExecution::new` directly.
An authorization hook that only exists at one ingress point leaves every
other caller of the decider runtime unenforced, and the decider crate itself
would have no way to reason about whether a given execution was ever checked.

### Put the principal on every `Decider`-implementing command struct

Rejected. `Decider::decide` takes `&self` as the command and already defines
`stream_id()`; adding a principal field to every command type conflates
domain payload (business intent) with cross-cutting authorization context,
forces every decider author to remember to carry and validate it, and cannot
be composed, tested, or swapped independently of the domain type.

## Non-Goals

- Defining a policy language (SpiceDB, CEL, Rego, or otherwise). This ADR
  defines the extension point a policy engine plugs into, not the engine.
- Replacing AAuth or changing its wire tokens; the `principal` claim's
  optional-string shape is AAuth's own pinned draft shape and out of scope
  here.
- Authorizing stream reads or snapshot reads/writes independent of command
  execution. Only the `decide` entry point is gated.
- Specifying how an application composes multiple authorizers (allow-list,
  policy callout, or otherwise). One trait, one hook per execution.

## Consequences

- `CommandExecution` and `WasmCommandExecution` gain a new builder slot and,
  once populated, a new phase in the execution pipeline; the default no-op
  authorizer keeps every existing call site compiling and behaving unchanged.
- `CommandError`/`WasmCommandError` gain an `Unauthorized` variant, additive
  but breaking for exhaustive matches on those enums, consistent with how
  every other execution phase already gets its own variant.
- Enforcement is opt-in per call site. This ADR does not retroactively close
  the "anyone can submit any command" gap the audit identified; it closes it
  only where a caller adopts an authorizer.
- Authorization becomes a distinguishable phase in logging and metrics,
  separate from a domain rejection (`Decide`) or an infrastructure failure
  (`Append`), matching this crate's existing philosophy of phase-tagged
  errors.
- Gets harder: the authorizer hook runs on every command execution, including
  hot paths, so a slow or blocking authorizer implementation directly adds
  latency to every command; this ADR does not mandate a cost bound on
  implementations.

## References

- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [Event Metadata](../architecture/event-metadata.md)
