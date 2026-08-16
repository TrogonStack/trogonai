---
number: "0026"
slug: command-authorization-principal
status: accepted
date: 2026-07-15
---

# ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution

## Context

`trogon_decider_runtime::execution::CommandExecution` is the single entry
point every native command execution goes through: it loads history, replays
it, calls `Decider::decide`, encodes the resulting events, and appends them.
Its builder carries an [event](../glossary/event) store, the command, an optional [write
precondition](../glossary/write-precondition), an optional [snapshot](../glossary/snapshot) policy, an event id generator, and
`headers: Headers` -- a freeform, caller-supplied metadata bag. Nothing in
that builder, nothing in the `Decider` trait itself, and nothing in the [WASM](../glossary/wasm)
mirror (`trogon_decider_wasm_runtime::execution::WasmCommandExecution`, which
has the identical shape) represents who is submitting a command or what they
are allowed to do. `decide` runs against decider state and the command
payload only.

`docs/architecture/event-metadata.md` is explicit that the runtime should not
derive headers generically: an application that wants a fixed header
([tenancy](../glossary/tenant), correlation, and by the same reasoning, caller identity) must build
it itself before calling `CommandExecution::with_headers`. In practice this
means the closest thing to "who acted" today is whatever string an
application chooses to put in a header, which `decide` never sees and which
carries no authorization semantics -- it is envelope metadata for storage, not
an input the runtime checks before evaluating a decision.

Command execution is also reached from more than one direction. The [A2A](../glossary/a2a)
gateway resolves caller identity at ingress
([ADR#0017](./0017-aauth-agent-authentication.md)), but internal callers such
as `trogon-scheduler`'s worker processor construct `CommandExecution::new`
directly, off the gateway path entirely. Any authorization hook that only
lives at the gateway leaves every non-gateway caller of `CommandExecution`
unenforced.

[AAuth](../glossary/aauth) itself is not a ready-made carrier for this. Its three-party auth token
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
language. It also carries the directed-principal hint Decision 4 describes,
in its own field, so a value that was never verified cannot be mistaken for a
claim that was.

### 2. An authorizer trait hook that runs before `decide`

Add a `CommandAuthorizer<C>` trait whose method takes the principal and the
command and returns either `Ok(())` or a typed denial. The command is a plain
generic parameter rather than a `Decider` bound: the WASM path's command is a
`CommandEnvelope`, which is not a `Decider`, so a `Decider` bound would make
Decision 3 unimplementable and split the trait in two.

`CommandExecution`/`WasmCommandExecution` gain an optional builder slot for an
authorizer, defaulting to a `WithoutAuthorization` no-op -- the same opt-in
shape `WithoutSnapshots` and `WithoutAdmission` already use. The default is
named for the absence of a policy rather than for a permissive one: "allow
all" reads like a decision somebody made, and nothing here was decided.
`WithoutAuthorization` preserves runtime behavior only: existing call sites
keep building and behaving exactly as they do today, but the `Unauthorized`
variant Decision 5 adds to the shared error enums is source-breaking for any
exhaustive match on them, including in consumers that never configure an
authorizer.

The trait carries two methods rather than one. Implementations write
`authorize`, which is only ever handed a principal that exists. The runtime
calls `authorize_execution`, whose default turns an absent principal into a
denial before `authorize` is consulted. Decision 5's fail-closed rule is
therefore the behavior an implementation gets by writing nothing, rather than
a rule each implementation is trusted to remember.

### 3. Both native and WASM dispatch paths, at the same point

The check runs once per execution on both paths, immediately after the
admission permit and before anything else: before the snapshot read, the
stream read, the guest instantiation, the replay, and `decide`. It sits
outside the conflict-retry loop, so a retried command is authorized exactly
once however many attempts it makes. A denial therefore costs one call and
nothing else on either path, and the guarantee the two paths offer is the
same guarantee rather than two path-specific ones.

The price of that placement is what the authorizer is handed: the principal
and the command, not the target stream and not the replayed state. On the
native path an authorizer that needs the stream calls `Decider::stream_id` on
the command it already has. On the WASM path the stream id is a guest-computed
value that does not exist until the guest has been instantiated and called, so
a WASM authorizer decides from the command envelope alone. Running the guest
first to hand an authorizer a stream id would mean paying guest instantiation
and [fuel](../glossary/fuel) for a command that is about to be denied, which
is the cost the placement exists to avoid.

### 4. Composing with AAuth given its optional-string principal

The gateway (or any other ingress boundary that already runs AAuth
verification) is where a verified identity becomes a `CommandPrincipal`: the
PoP-verified agent's `sub`/`cnf.jwk` thumbprint maps to an agent principal,
and an `aa-auth+jwt`'s `principal` string, when present, is carried as an
opaque hint attached to that principal rather than trusted as a scoped claim
on its own. This [ADR](../glossary/adr) does not change AAuth's wire shape or fix the
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

### Authorize in an application-owned wrapper, outside the decider crates

Rejected, with the reasoning in Resolved Question 1. The decider crate family
is reusable and business-agnostic, and its admission bar is that a concern is
domain-level and makes sense for every consumer with no business context;
authorization is application-level policy. Under that boundary the natural
shape is a wrapper the application composes around `CommandExecution`
(authorize, then execute): no hook in the shared crates, no new builder slot,
no new variant on the shared error enums. Its cost is the property Decisions
1-3 buy by moving the hook inside: the runtime itself can no longer state that
a given execution was ever checked, and every non-gateway caller must remember
to go through the wrapper.

An illustrative sketch of the wrapper shape, in application code only.
Every name here is placeholder, and whether the pieces live per-app or in a
shared app-side crate is open; the sketch exists to make the placement
discussion concrete, not to decide it:

```rust
// Application-owned. The decider crates never see these types.
pub struct Principal { /* kind, id, claims: whatever the app decides */ }
pub struct Denied { pub reason: String }

pub trait Authorize<C> {
    fn authorize(&self, principal: &Principal, command: &C) -> Result<(), Denied>;
}

pub enum AppError<E> {
    Denied(Denied),
    Execution(E),
}

// The application's one entry point for executing a command: authorize,
// assemble every required boundary input (audit headers among them), then
// run the untouched CommandExecution.
pub struct CommandBoundary<E, A> {
    event_store: E,
    authorizer: A,
}

impl<E, A> CommandBoundary<E, A> {
    pub async fn execute_command<C>(
        &self,
        principal: &Principal,
        command: &C,
    ) -> Result<ExecutionOutcome, AppError<ExecutionError>>
    where
        C: Decider, // plus the store bounds CommandExecution already requires
        A: Authorize<C>,
    {
        self.authorizer.authorize(principal, command).map_err(AppError::Denied)?;
        let headers = required_headers(principal, command)?;
        CommandExecution::new(&self.event_store, command)
            .with_headers(headers)
            .execute()
            .await
            .map_err(AppError::Execution)
    }
}
```

Two properties of the sketch worth naming for the discussion. The
`required_headers` function is deliberately general: it assembles every
input the application requires at the boundary (audit identity among them),
not an audit-only helper. And if the application keeps its raw event-store
handle private to the module defining this entry point, the discipline cost
above shrinks from team convention to module visibility: the only ergonomic
path to an append goes through the gate.

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

## Resolved Questions

1. **Placement.** Resolved in favour of the in-crate seam. The boundary rule
   the wrapper argument invokes is about *policy*, and the seam holds no
   policy: it owns a principal type, a trait, and a no-op default, the same
   three pieces [ADR#0028](./0028-decider-admission-control-and-backpressure.md)
   accepted for admission control, which is equally an operational concern the
   crate refuses to have an opinion about. What the wrapper cannot give is the
   property the audit asked for. A wrapper's guarantee holds only for callers
   who go through it, is unverifiable by the runtime, and is unavailable to
   consumers outside the application that defines it, including
   `trogon-scheduler`'s worker processor and any WASM host. With the hook
   inside, "was this execution authorized" is answerable from the execution's
   own type: an execution parameterized on `WithoutAuthorization` was not, and
   one parameterized on anything else was. The wrapper stays viable as a
   composition on top for applications that want one; it is no longer the only
   way to get a check.

2. **Error surface.** Resolved: on the shared enums, following the placement.
   `CommandError`/`WasmCommandError` are phase taxonomies, and authorization is
   a phase. The source-breaking cost is real and is the same cost `Overloaded`
   already imposed when ADR#0028 was accepted; paying it twice in one release
   cycle is cheaper than a denial that arrives as an opaque wrapper error a
   phase-matching consumer cannot classify.

3. **Upstream protocol churn.** Resolved: the coupling is confined to the
   ingress mapper and does not reach the runtime. `CommandPrincipal` is
   deliberately not an AAuth type and names nothing from that protocol. It
   carries a kind, a stable identifier, a claim set, and a separate
   `directed_principal` field holding the `aa-auth+jwt` `principal` string as
   an explicitly untrusted hint, never as a claim. If
   [ADR#0017](./0017-aauth-agent-authentication.md) re-pins its Internet-Draft
   revision, what changes is Decision 4's mapping at the boundary that
   performs the verification; the principal type, the trait, and every
   authorizer implementation are unaffected.

The resolutions above were reached at acceptance time and have not been
reviewed by a second party. Only the seam described in Decisions 1-3 and 5 is
implemented. Decision 4 remains unimplemented: no ingress boundary maps an
AAuth identity into a `CommandPrincipal` yet, so in practice every principal
the runtime sees today is one a caller constructed directly.

## Consequences

- `CommandExecution` and `WasmCommandExecution` gain a new builder slot and,
  once populated, a new phase in the execution pipeline; the default no-op
  authorizer keeps every existing call site behaving unchanged at runtime.
- `CommandError`/`WasmCommandError` gain an `Unauthorized` variant, additive
  but immediately source-breaking for exhaustive matches on those enums
  whether or not a caller ever configures an authorizer, consistent with how
  every other execution phase already gets its own variant.
- Enforcement is opt-in per call site. This ADR does not retroactively close
  the "anyone can submit any command" gap the audit identified; it closes it
  only where a caller adopts an authorizer.
- Authorization becomes a distinguishable phase in logging and metrics,
  separate from a domain rejection (`Decide`) or an infrastructure failure
  (`Append`), matching this crate's existing philosophy of phase-tagged
  errors. The `decision_outcome` attribute gains a `denied` member alongside
  `shed`, so a denial is countable without being confused for either a domain
  rejection or a fault.
- An authorizer sees the principal and the command, never the target stream's
  state. That is a deliberate limit of the placement, not an oversight: a
  policy that must inspect stream state to decide is expressing a domain rule,
  and a domain rule belongs in `decide`, where a rejection is already a
  first-class outcome.
- The seam ships; the ingress half does not. Decision 4's AAuth mapping is
  unimplemented, so nothing yet produces a `CommandPrincipal` from a verified
  identity. Until something does, a configured authorizer is only as
  trustworthy as whatever constructed the principal handed to it.
- Gets harder: the authorizer hook runs on every command execution, including
  hot paths, so a slow or blocking authorizer implementation directly adds
  latency to every command; this ADR does not mandate a cost bound on
  implementations.

## References

- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [Event Metadata](../architecture/event-metadata.md)
