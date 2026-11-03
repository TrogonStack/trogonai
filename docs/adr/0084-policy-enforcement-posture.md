---
number: "0084"
slug: policy-enforcement-posture
status: draft
date: 2026-11-02
---

# ADR#0084: Every Policy Enforcement Point Enforces by Default and Refuses to Start Unenforced

## Context

[ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
decides what a policy binding says: absence of a permit is denial, and a hold
fails closed. It does not decide what an enforcement point does when it has no
policy to read, when the policy it was told to read does not load, or when a
request matches none of the rules it has. The points that exist today answer
all three by letting the request through.

The A2A gateway is the one point that serves traffic. It has four layers, and
each is off until an environment variable turns it on: Tier-1 SpiceDB
(`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), Tier-1 declarative
(`A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED`), Tier-2 CEL
(`A2A_GATEWAY_TIER2_CEL_ENABLED`) and Tier-3 redaction
(`A2A_GATEWAY_TIER3_REDACTION_ENABLED`). With none of them set,
`gateway_policy_stack_from_env` returns `GatewayPolicyStack::noop()`
(`rsworkspace/crates/a2a/a2a-gateway/src/runtime/policy_stack.rs:56-62`),
`run_with_config` builds the Noop Tier-1 gates (`runtime.rs:69-75`), and the
process subscribes to the ingress wildcard and forwards every request to the
agent it names. Since the default build began to serve, that is a gateway an
operator can start by accident with no policy at all and no message saying so.

One half of the failure mode is already closed. When a layer the operator
turned on cannot be brought up, `PolicyStackError` stops the process instead
of falling back to Noop: a missing bundle directory, an unusable signing key,
a substrate that cannot load, a Tier-2 bundle that does not compile or holds no
rules. The Tier-1 declarative loader likewise refuses a directory that does not
exist (`tier1_declarative/loader.rs:72-79`). The other half is open:

- **An unmatched request is allowed.** `RealTier1DeclarativeGate::evaluate`
  ends in `Tier1DeclarativeDecision::Allow { rule: None }` when no rule matches
  (`policy/tier1_declarative/evaluator.rs:121`), and the audit record names it
  `gateway.tier1.declarative.no_match_default_allow` (line 309). A directory
  that holds no `*.tier1.toml` file is a valid empty bundle (`loader.rs:76-79`),
  so a layer that looks enabled allows everything. The reference bundles in
  `rsworkspace/crates/a2a/a2a-pack/policies/` work only because they carry a
  catch-all deny, as `per-method-allowlist` does at priority 100; a bundle
  without one inherits the allow.
- **A layer that is off is indistinguishable from a layer that is not needed.**
  `NoopTier1DeclarativeGate` returns the same `Allow { rule: None }`
  (`evaluator.rs:83-86`), and the dispatcher does not tell the operator which
  layers are enforcing.
- **The decider host admits by default.** `CommandExecution` defaults to
  `WithoutAuthorization`
  (`rsworkspace/crates/decider/trogon-decider-runtime/src/execution.rs:1249`).
  [ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
  Decision 2 closed the absent-authorizer case in the NATS host by making
  `AdmitAnyPrincipal` its default
  (`rsworkspace/crates/decider/trogon-decider-nats-server/src/runtime.rs:207`,
  `authorizer.rs:16`). That authorizer admits every principal the subject
  yields, so a deployment with no policy of its own still runs every command
  from every connection the allow list lets publish.

The posture is also written down in the wrong direction. [ADR#0017](./0017-aauth-agent-authentication.md)
Decision 6 names `off` as the AAuth default, and
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
Decision 4 already reverses that for the HTTP edge. Nothing states the rule
for the other points, and two more are coming (an egress proxy and an MCP
boundary) that would each have to choose a default again.

[ADR#0077](./0077-api-key-authorization-model-and-rate-limits.md) Decision 8
also chooses fail open, for the rate limiter on ordinary keyspaces. That is a
fairness control, not an authorization decision, and Decision 7 below says
why it stays.

## Decision

### 1. Every enforcement point defaults to Enforce, and a point with no enforcing layer refuses to start

An enforcement point is a process that receives a request and decides whether
it proceeds, by reading a policy: each tier of the A2A gateway, the authorizer
of the decider host, and the egress and MCP boundaries when they exist. Each
has one or more layers, and each layer has a mode (Decision 4).

A point starts only if at least one of its layers is in Enforce. Otherwise
startup fails with a typed error in the manner of
`RuntimeError::NoPolicyConfigured`, whose message names the environment
variables that would enable a layer. The process exit code is 1. No code path
falls back to a Noop or allow-all layer to satisfy the check, and a layer
that has no configuration is `off`, which the check does not count.

### 2. A layer the operator enabled either loads or the process refuses to start

The rule `PolicyStackError` implements for the A2A gateway is the rule for
every point. Once a layer's mode is anything but `off`, a missing bundle, an
unreadable directory, an invalid key or a substrate that cannot load stops the
process. No variant of that failure may degrade to a weaker stack, and no
log line substitutes for the refusal. A layer that fails after startup, such
as a hot reload, keeps its last good policy or denies every request; it never
admits.

An enabled layer that loads nothing is also a load failure. A Tier-1
declarative directory without a rule file, and a Tier-2 bundle without rules
(already refused as `Tier2NoRules`), are refused for the same reason: the
operator asked for a policy and supplied none.

### 3. A request that matches no rule is denied

In a layer that evaluates rules, a request that no rule matches is denied with
the layer's own refusal code (`-32803` for Tier-1 declarative) and an audit
record whose rule is `gateway.tier1.declarative.no_match_default_deny`. A bundle
has no field that sets an allow for unmatched requests. A policy that wants
broad access states it as a visible catch-all allow rule with an id, which the
audit record then names. A catch-all allow only means the layer does not
refuse; it never admits an action that a policy binding governs and no permit
grants. The same holds for a permit-shaped binding of
[ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md):
absence is denial in every layer that consumes it.

Changing an existing allow-by-default into a deny ships first as DryRun. The
layer that gains the new default runs it in DryRun for one release, recording
`would_deny` on each request that the new default would have refused, and the
following release flips it to Enforce. This rule applies to any point that has
a deployment to protect; Decision 8 says which points that is today.

### 4. Each layer has a mode, `EnforcementMode { DryRun, Enforce }`

The mode of a layer is one of three values in `A2A_GATEWAY_<LAYER>_MODE`:
`off`, `dry_run` and `enforce`, with `<LAYER>` one of `TIER1_SPICEDB`,
`TIER1_DECLARATIVE`, `TIER2_CEL`, `TIER3_REDACTION` or `AAUTH`. `off` means the
layer is not configured; it is not a mode of the enum, because a layer that is
not configured has no `EnforcementMode`. `shadow` is accepted as an alias of
`dry_run`, which is the name AAuth uses today. The existing `*_ENABLED`
boolean variables read as `enforce`, and a boolean and a mode variable that
disagree are a startup error. Other points name their variables the same way,
`<POINT>_<LAYER>_MODE`.

- **Enforce** evaluates the policy and refuses a request it denies, with the
  layer's refusal code, forwarding nothing.
- **DryRun** evaluates the policy and records an outcome tagged `would_deny`
  (`gateway.<layer>.would_deny` in the audit record's rules) when Enforce would
  have refused, then forwards the request. An evaluator error in DryRun is
  recorded and forwarded; in Enforce it denies.

DryRun is an explicit choice for each layer. No layer, point or build
defaults to it, with the one dated exception of Decision 8. The process logs a
startup warning for every layer that is in DryRun.

For the check in Decision 1, a layer in DryRun is not an enforcing layer. A
point whose layers are all in DryRun, such as an operator measuring a first
policy, starts only when `A2A_GATEWAY_ALLOW_DRY_RUN_ONLY` is set, and logs a
warning that names it. This is the one place the check accepts a non-enforcing
layer without the escape hatch of Decision 5.

### 5. One escape hatch, for local development

`A2A_GATEWAY_ALLOW_ALL_UNSAFE` lets a gateway with no enforcing layer start.
Other points follow the pattern `<POINT>_ALLOW_ALL_UNSAFE`. With it set, the
point logs an `error` line once at startup and a `warn` line for every request
it admits, and each audit record carries the rule `gateway.unsafe.allow_all`.
It does not weaken Decision 2: a layer the operator enabled that fails to load
still stops the process.

A release build rejects the variable at startup, with the same typed error as
a missing policy, unless the explicit development flag that
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
Decision 4 requires for Shadow and Off at the HTTP edge is also set. This
record adds no second development flag and does not name that one; the
implementation of ADR#0082 Decision 4 does, and every point reads the same
variable. The same flag gates `A2A_GATEWAY_ALLOW_DRY_RUN_ONLY` in release
builds.

### 6. `--check-config` and the startup line

A point accepts `--check-config`. It loads every layer exactly as startup
does, prints one line per layer with its mode and the policy source it loaded,
and exits 0 when startup would succeed and non-zero on any error, including
the missing-policy refusal of Decision 1. It subscribes to nothing and serves
nothing.

At startup a running point logs one `info` line that lists every layer and its
mode, `off` included, so the posture of a process is visible in its first
log lines and in a single search. Each layer in DryRun also gets the warning of
Decision 4.

### 7. What is not an enforcement point

This record covers a decision whose outcome is that a request proceeds or is
refused because of who or what it is. It does not cover a control whose
outage should not become the service's outage. The rate limiter of
[ADR#0077](./0077-api-key-authorization-model-and-rate-limits.md) Decision 8
fails open on ordinary keyspaces, emits a metric for the window, and stays as
it is. Where such a control guards a privileged operation, that ADR already
fails it closed. A control that is added to a point and fails open must say so
in its own ADR; a point's authorization layers never do.

### 8. Migration

Nothing deploys the A2A gateway, so it takes Decisions 1 to 6 with no grace
release: its first release already refuses to start with no enforcing layer,
and the Tier-1 declarative default denies from that release.

Decider authorizers get exactly one DryRun release. During it, the decider
host's authorizer runs in DryRun by default, which is the single exception to
Decision 4: a host whose deployment supplies a policy records `would_deny` and
admits, and a host without one starts with the Decision 4 warning instead of
the Decision 1 refusal. The reason is that the commands the host serves come
from principals that no policy has yet been run against, and a flip to Enforce
with no measurement would stop commands nobody knew were unauthorized. In the
release after it the host's authorizer is in Enforce, and a host with no
authorizer of its own refuses to start unless `DECIDER_ALLOW_ALL_UNSAFE` is
set under Decision 5. `AdmitAnyPrincipal` stays as the type that escape hatch
installs; it is no longer a default.

Future points (egress, MCP) ship in Enforce from their first release.

What this changes in [ADR#0017](./0017-aauth-agent-authentication.md):

- Decision 6. The mode no longer defaults to `off`. Together with
  [ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
  Decision 4, which moved the layer to the HTTP edge and made Enforce its
  default, the AAuth mode variable follows this record's per-layer contract:
  `off` is legal and counts as no layer, `dry_run` is the existing `shadow`
  under its new name, and neither is accepted in release builds without the
  development flag of Decision 5.
- Decision 6, "no silent fallback to `Off` on misconfiguration". This stays
  and becomes Decision 2 of this record for every layer.

What this changes in [ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md):

- Decision 2, "A deployment without its own policy gets the host's default
  authorizer, `AdmitAnyPrincipal`". During the DryRun release it still starts.
  After it, such a host refuses to start unless `DECIDER_ALLOW_ALL_UNSAFE` is
  set, and `AdmitAnyPrincipal` is what that variable installs.
- Decision 4. This record does not name the development flag that decision
  requires; it reuses it for the escape hatch of Decision 5.

[ADR#0077](./0077-api-key-authorization-model-and-rate-limits.md) is not
amended. Its fail-open default is for a rate limiter, not an enforcement
point (Decision 7), and the failure window stays visible through the metric
that ADR already requires.

## Alternatives Considered

### Default to DryRun for one release and flip to Enforce later

Rejected for the A2A gateway. A default that is later flipped is an allow
default for as long as it exists, and the gateway has no deployment whose
traffic needs a grace period. The cost of the flip is paid by the first
operator who starts the process and reads no changelog. Accepted only for the
decider host (Decision 8), where a deployment exists and no policy has been
measured against its principals.

### Each point opts in to Enforce

Rejected. This is today's posture: a point that has not been configured
admits, and an operator learns that from an incident. A rule that each point
must choose Enforce also repeats for the egress proxy and the MCP boundary,
where the first implementation is the one least likely to remember.

### Warn when no policy is loaded and keep serving

Rejected. A warning on an otherwise healthy process is a line nobody reads in
a fleet of replicas, and the gateway forwards every request while it is
printed. [ADR#0017](./0017-aauth-agent-authentication.md) Decision 6 already
chose a typed startup failure over a silent fallback for misconfiguration;
no policy at all is the same misconfiguration.

### Make DryRun count as enforcing for the startup check

Rejected as the default. A point in which every layer is in DryRun forwards
every request, so the check would pass for a process that enforces nothing.
Measurement stays possible through `A2A_GATEWAY_ALLOW_DRY_RUN_ONLY`, which
names what the operator is doing.

### A per-bundle `default_effect` field with `allow` as a valid value

Rejected. It keeps the allow-by-default behavior one field away and makes the
audit record say `default` instead of a rule id. A catch-all allow rule is the
same policy with a name that can be reviewed, versioned and searched.

### Keep the Noop stack as the production fallback for "layer off"

Rejected. The Noop stack is the same Allow as a layer that loaded and approved,
and the audit record cannot tell them apart. Marking a layer `off` is a
statement of intent that Decision 6 prints; a Noop gate that answers as if it
had evaluated is not.

## Consequences

- A gateway with no layer set stops with `NoPolicyConfigured` where it serves
  today. Every environment that starts one, the dev compose stack included,
  sets at least one layer to `enforce` or sets `A2A_GATEWAY_ALLOW_ALL_UNSAFE`
  under the development flag, and tests that start a gateway do the same in
  their fixture.
- Tier-1 declarative bundles that rely on the allow default must add a
  catch-all allow rule before they load as intended. The shipped bundles under
  `a2a-pack/policies/` and their README are reviewed against Decision 3, and
  the reference bundle that has no catch-all is changed or documented as
  deny-by-default.
- Tests built on the `no_match_default_allow` audit string give way to tests
  that assert denial of an unmatched request and a refused start.
- The decider host's call sites that construct an execution with
  `WithoutAuthorization` are listed by the same check that listed the
  absence of an authorizer in [ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md).
  An architecture test fails when a production execution lacks an explicit
  authorizer.
- Operators get three new things to rely on: the startup line, the
  `would_deny` audit tag, and `--check-config` in CI and deploy scripts.
  Each is a stable surface once released.
- The credential release policy has the same shape (its unconfigured host and
  service restrictions are `Unrestricted` by default,
  `rsworkspace/crates/platform/trogon-gateway/src/credential/commands/domain/runtime_release_policy.rs`).
  It is a decision about a value object, not a layer with a mode, and is not
  decided here.
- The egress proxy and MCP boundary inherit this posture and cannot choose
  differently without superseding this record.

## References

- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [ADR#0071: What an Agent May Reach, How Far It May Go, and Whether It May Run at All Are Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
- [ADR#0077: API Key Authorization Model and Rate Limits](./0077-api-key-authorization-model-and-rate-limits.md)
- [ADR#0082: Inside the Bus the Principal Is Derived from the Subject, and Proof of Possession Is Enforced Only at the External Edge](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
