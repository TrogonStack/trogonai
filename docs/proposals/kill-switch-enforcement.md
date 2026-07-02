# Proposal: Kill-switch enforcement at the NATS subject-permission layer

Status: draft
Tracks: WI-19 (MS_AGENT_GOV_TOOLKIT_WORKITEMS.md)
Kind: enforcement design, no implementation in this change. The domain core
and the KV projection this design assumes are implemented in
`rsworkspace/crates/trogon-kill-switch`; the enforcement wiring described
below (auth-callout integration, stream provisioning, revocation path) is not.

## Why

AGT's kill switch
(`agent-governance-python/agent-hypervisor/src/hypervisor/security/kill_switch.py`)
is a `threading.Lock`-guarded, in-process registry. `KillSwitch.kill()` looks
up a termination callback the target agent registered for itself, invokes it
in a daemon thread with a five-second timeout, and unconditionally marks the
agent unregistered whether or not that callback ran, timed out, or raised.
The kill *record* is always durable in the sense that a `KillResult` is
returned and logged, but the kill's *effect* is not: a hung, malicious, or
already-compromised agent is exactly the agent least likely to run its own
termination callback honestly, and AGT's design has no fallback when it
doesn't. This is a structural gap, not a bug: an in-process callback registry
has no way to act on an agent that no longer cooperates with the process that
is trying to kill it.

TrogonAi's primitives close this gap by moving enforcement out of the killed
agent's control entirely. `trogon-kill-switch` already models `Kill`/`Revive`
as a decider aggregate (see `rsworkspace/crates/trogon-kill-switch`) whose
current state is projectable into a NATS JetStream KV bucket. This document
specifies the remaining piece: enforcing a kill by revoking the killed
agent's NATS subject permissions at the connection-authorization layer, so
the agent cannot publish or consume regardless of whether its own process
cooperates. This is exactly the design MS_AGENT_GOV_TOOLKIT.md section 24.5
sketches: "enforce at the NATS subject-permission layer by revoking
publish/consume on the killed agent's subject prefix."

The `KillReason` taxonomy and the general shape of "a kill switch records
who was killed, why, and when" mirror AGT's `hypervisor.security.kill_switch`
module and `docs/specs/AGENT-HYPERVISOR-EXECUTION-CONTROL-1.0.md` section 12.
No AGT source is copied into this repository; only the described concept is
re-expressed as a Rust decider aggregate, and hardened by moving
enforcement from a cooperative in-process callback to a NATS-level
permission revocation the killed agent cannot opt out of.

## Why four `KillReason` variants, not AGT's six

AGT's `KillReason` enum has six values: `BEHAVIORAL_DRIFT`, `RATE_LIMIT`,
`RING_BREACH`, `MANUAL`, `QUARANTINE_TIMEOUT`, `SESSION_TIMEOUT`.
`trogon_kill_switch::KillReason` (`rsworkspace/crates/trogon-kill-switch/src/kill_reason.rs`)
carries only the first four. The two omitted values are AGT session-lifecycle
concepts:

- `QUARANTINE_TIMEOUT` fires when AGT's hypervisor holds an agent in a
  time-boxed quarantine state and the quarantine window expires without a
  human decision. TrogonAi has no quarantine state machine today; there is
  nothing in this codebase for that reason to describe.
- `SESSION_TIMEOUT` fires when AGT's per-session idle/lifetime timer expires.
  TrogonAi has no equivalent session-lifetime concept at the point this
  decider was written.

Both are timeout-driven variants of a session/quarantine model that does not
exist yet in TrogonAi. Adding them now would mean two `KillReason` values
with no caller that can ever construct them, which is exactly the kind of
speculative surface `crates/AGENTS.md`'s "validated at construction, no
unrepresentable-but-declared states" discipline argues against. The fix if
and when TrogonAi grows a quarantine or session-lifetime concept is
mechanical: add the corresponding variant to `KillReason`, extend
`KillReason::as_str`/`parse`, and add the round-trip test, all in one file
(`kill_reason.rs`). This is not a closed taxonomy; it is scoped to what this
codebase can currently cause.

## What already exists (`trogon-kill-switch` domain core)

For context on what enforcement builds on top of. All of the following is
implemented, not proposed:

- `Kill`/`Revive` commands and `AgentKilled`/`AgentRevived` events, modeled
  as a `trogon_decider::Decider` the same way
  `trogon-scheduler-domain`'s `PauseSchedule` models a schedule mutation:
  one aggregate instance per `AgentId`, `decide` rejects the command that
  would be a no-op (`Kill` on an already-killed agent, `Revive` on an
  already-alive agent) with a typed error rather than silently succeeding,
  and `evolve` is a pure state fold.
- `KillSwitchEvent`'s wire codec (`event_wire.rs`) implementing
  `trogon_decider::event::{EventType, EventEncode, EventDecode}`, the same
  codec boundary `trogon-decider-nats`'s `JetStreamStore` uses to persist
  and rehydrate aggregates from a JetStream stream.
- A KV projection (`projection/`) shaped like `a2a-nats`'s
  `KvCatalogStore` (`rsworkspace/crates/a2a-nats/src/catalog/store.rs`):
  one JetStream KV bucket (`KILL_SWITCH_STATE`), one key per `AgentId`,
  tombstone-aware create-vs-update, bounded conflict retry. It has no
  consumer loop wired to it; `KillSwitchProjectionStore::put` is a primitive
  a future consumer calls, not a running process.

None of this depends on NATS account/permission APIs. Nothing above can
currently affect whether a killed agent's NATS connection can publish or
subscribe. That is the whole subject of this document.

## The enforcement problem, precisely

A `Kill` command decided and appended to `KILL_SWITCH_EVENTS`, and even
projected into `KILL_SWITCH_STATE`, is a fact about the system's records. It
is not, by itself, a change to what the killed agent's live NATS connection
is authorized to do. Two independent existing connections are unaffected by
an event being written:

1. The killed agent's own already-established NATS connection keeps
   whatever publish/subscribe permissions it was issued at connect time.
   NATS does not re-check permissions against anything after the initial
   `CONNECT`/`AUTHORIZATION` handshake for a live connection.
2. A brand-new connection attempt from the killed agent, if authorized by
   the same static credential it used before, would be issued the same
   permissions again, because nothing about credential validation
   consulted kill-switch state.

Closing both requires two distinct mechanisms: revoking the *live*
connection, and refusing to *reissue* permissions on the next connect. NATS
gives a mechanism for each.

## Design: enforcement via `a2a-auth-callout`

This repository already has an auth-callout implementation,
`rsworkspace/crates/a2a-auth-callout`, that mints per-caller NATS
permissions at connect time (`IssuedPermissions`, `SubjectAclTemplate`,
`SubjectAclContext` in `a2a-auth-callout/src/permissions.rs`) via NATS's
connection-authorization callout mechanism. This is the natural integration
point: kill-switch state becomes an input to permission issuance, not a
separate enforcement path bolted on afterward.

### 1. Refusing reissue: gate `IssuedPermissions` on projected kill state

`a2a-auth-callout`'s dispatcher (`a2a-auth-callout/src/dispatcher.rs`)
resolves a caller's identity and materializes an `IssuedPermissions` for it
on every connect attempt. The enforcement point is a single additional read
before permissions are materialized: given the connecting caller's
`AgentId`, call `KillSwitchProjectionStore::get(&agent_id)`. If the
projected state is `KillSwitchState::Killed { .. }`, the callout denies the
connection outright (an empty/deny-all `IssuedPermissions`, or a hard
`DenialCategory`/`DenialReason` rejection, whichever `a2a-auth-callout`'s
existing denial vocabulary already models for "caller not authorized")
instead of materializing the caller's normal ACL template. A revived agent
(`AgentRevived` projected back to `KillSwitchState::Alive`) is
indistinguishable at this check from an agent that was never killed, so
`Revive` requires no separate re-authorization step.

This closes case 2 above: every new connection attempt is checked against
current kill state, so a killed agent cannot reconnect its way back to
permissions. The KV projection is exactly the right read for this check
because it is a single bounded lookup keyed by `AgentId`, not a stream
replay, which matters on a hot path every connection attempt goes through.

### 2. Revoking the live connection: subject-prefix permission narrowing

Case 1 needs an action against an already-connected client, which NATS
supports through the account/subject-permission model rather than through
the auth callout (the callout only runs at connect time). The mechanism:
every issued `IssuedPermissions` for an agent should scope its publish and
subscribe subjects under that agent's own subject prefix (an `AgentId`-keyed
segment of the subject space, the same shape `IssuedPermissions::default_for_caller`
already uses for `_INBOX.{caller}.>` and `a2a.push.{caller}.>`). Killing an
agent then becomes, operationally: revoke the specific permission grant
for that agent's subject prefix rather than trying to force-disconnect a
live TCP connection (NATS server does expose connection eviction, but
subject-permission revocation is preferable because it is authoritative at
the message-routing layer regardless of how many connections the killed
credential holds, including ones opened after the kill and before any
disconnect sweep runs).

Concretely, this means the same kill-switch consumer that projects
`AgentKilled`/`AgentRevived` into `KILL_SWITCH_STATE` (see "What remains" below)
is also the trigger for telling the NATS account server (or, if
`a2a-auth-callout` owns permission issuance dynamically per-connection
rather than through static NATS account permissions, the callout's own
permission cache) that the killed agent's subject-prefix grant is revoked.
The precise transport for that revocation (an account JWT update and
`nats-server` config reload signal, versus a callout-side cache invalidation
that takes effect on next permission check, versus explicit connection
eviction as a defense-in-depth backstop) is an implementation decision for
whoever wires this, not specified further here, because `a2a-auth-callout`'s
existing permission-issuance model is per-connect-time rather than
per-live-connection, and picking between "narrow permissions look-aside on
every publish" versus "evict and force reconnect through the gated path"
is a real design question with latency/complexity tradeoffs this proposal
does not have enough information to settle. What is specified is the
invariant enforcement must uphold: after an `AgentKilled` event is durably
appended, there must exist no live or newly issued NATS credential for that
`AgentId` with publish or subscribe access to that agent's subject prefix,
within whatever propagation delay the chosen revocation transport allows
made an explicit, monitored SLO rather than left implicit.

### 3. `KILL_SWITCH_EVENTS` stream

The stream `trogon-decider-nats`'s `JetStreamStore` would use to persist
`Kill`/`Revive` decisions is not yet provisioned in code (no stream-config
constant exists in `trogon-kill-switch` today; the crate only defines the KV
bucket config for the projection side). Specifying it precisely here per
this proposal's own scope discipline:

- Name: `KILL_SWITCH_EVENTS`, matching MS_AGENT_GOV_TOOLKIT.md section 24.5.
- Subjects: one subject per aggregate instance, following
  `trogon-decider-nats`'s stream-per-aggregate-instance convention (see
  `trogon-scheduler-domain`'s stream wiring for the pattern this should
  copy) with `AgentId` as the per-instance token, e.g. a subject shape like
  `kill_switch.events.{agent_id}`.
  `AgentId::parse` already rejects values (empty, over 256 characters,
  surrounding whitespace) that would be unsafe as either a decider stream
  id or a KV key; a NATS-token-safety check should be added if `AgentId` is
  ever allowed to contain characters `.`, `*`, or `>` that are meaningful to
  NATS subject matching, which the current validation does not exclude
  because no such value has been exercised in the token-safety test
  (`agent_id/tests.rs`'s `accepts_values_a_nats_token_would_reject` test
  documents this as an intentional permissiveness at the value-object layer,
  deferred to the stream-subject-construction layer instead).
- Retention: append-only, no explicit max age. A kill-switch audit trail is
  exactly the kind of record MS_AGENT_GOV_TOOLKIT.md section 24.5's
  companion audit-hash-chain sketch describes wanting to keep durable and
  tamper-evident; truncating it would undermine that goal, so this proposal
  recommends `Limits` retention with no `max_age` unless an operator-level
  retention policy is decided separately.
- Consumer: a durable JetStream consumer that folds every
  `AgentKilled`/`AgentRevived` event into two side effects: (a) call
  `KillSwitchProjectionStore::put` to keep `KILL_SWITCH_STATE` current, and
  (b) trigger the subject-permission revocation/reinstatement described
  above. Both are idempotent given the decider's own idempotency rules (a
  duplicate redelivery of `AgentKilled` re-projects the same state and
  re-issues the same revocation, which is a no-op), so at-least-once
  JetStream consumer semantics are sufficient; exactly-once is not required.

## Why this is structurally stronger than AGT's callback model

AGT's model: kill decision to enforcement action is a single in-process
function call to code the *target* agent registered, with a timeout as the
only defense against non-cooperation, and no way to distinguish "callback
ran and cleanly stopped the agent" from "callback ran and the agent kept
going anyway": `KillResult.terminated` only tells you whether the callback
returned before the timeout, not whether the agent actually stopped doing
anything.

This design's model: kill decision to enforcement action is a durable event
on a replicated log, consumed by infrastructure the target agent does not
control and cannot suppress (a JetStream consumer and a NATS connection
authorizer are both outside the killed agent's process boundary), acting on
a substrate (subject-permission grants) that the target agent's own
cooperation is irrelevant to. A killed agent that ignores the kill entirely
still cannot publish or subscribe on its subject prefix, because the thing
being changed is not something inside that agent's process. The trust
boundary AGT's design puts inside the agent being killed is, in this
design, outside it, at the NATS server / auth-callout layer, which is
exactly the "real process and network boundaries" MS_AGENT_GOV_TOOLKIT.md
section 24.6 identifies as TrogonAi's comparative strength.

The tradeoff is latency and complexity: AGT's callback fires and (if the
agent cooperates) takes effect immediately in-process, with no propagation
delay. This design's revocation takes effect on whatever cadence the
JetStream consumer and the chosen revocation transport allow, which is why
"What remains" above calls out that propagation delay as something that
must be an explicit, monitored SLO rather than an unstated assumption.

## What remains (explicitly out of scope here)

This document specifies enforcement; it does not implement it. Not done,
and not part of this change:

- The `KILL_SWITCH_EVENTS` stream is not provisioned in code. No
  `trogon-decider-nats` `JetStreamStore` wiring exists for `Kill`/`Revive`
  yet.
- No JetStream consumer folds `KILL_SWITCH_EVENTS` into
  `KILL_SWITCH_STATE`. `KillSwitchProjectionStore::put` exists and is
  tested; nothing calls it outside tests.
- No `a2a-auth-callout` integration exists. The dispatcher does not
  consult `KillSwitchProjectionStore::get` today, and `IssuedPermissions`
  has no notion of a per-agent subject prefix narrow enough to revoke
  independently of a caller's other permissions.
- No decision has been made on the live-connection revocation transport
  (account JWT update, callout-side cache invalidation, connection
  eviction, or some combination), beyond specifying the invariant it must
  uphold.
- No monitoring or SLO exists for kill-to-enforcement propagation delay.

Each of these is a separable follow-up with its own design questions
(stream provisioning touches `trogon-decider-nats` wiring conventions;
auth-callout integration touches `a2a-auth-callout`'s denial vocabulary and
its own test-support mocks; the revocation transport choice needs NATS
account-server operational input this proposal does not have). Bundling
them into this change would trade the domain core's current correctness
for breadth this proposal is deliberately not attempting.
