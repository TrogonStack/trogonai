---
number: "0086"
slug: platform-audit-record-and-must-audit-class
status: draft
date: 2026-11-03
---

# ADR#0086: Platform Audit Records Share One Envelope, and an Enforcing Point That Cannot Durably Write a Must-Audit Record Refuses the Action

## Context

The platform refuses requests, releases credentials and changes policy, and none
of those acts leaves a record that survives the process that made it. What
exists is a vocabulary and a best-effort emitter.

**The A2A gateway audits on a path that can lose any record.** `AuditEnvelope`
(`rsworkspace/crates/a2a/a2a-nats/src/audit/envelope.rs:103-113`) holds the agent,
method, request id, start time, latency, an `ok` or `err` outcome and a
fingerprint of the request params, plus optional fields for trace id, rules
fired, subject rewrites, the SpiceDB consistency token, caller id and source, and
the Tier-1 and Tier-3 verdicts (`:81-99`). `NatsAuditEmitter` publishes it with
core NATS `publish_with_headers` and turns a failure into a `tracing::warn!`
(`a2a-nats/src/audit/emitter.rs:64-71`, `:83-90`). The gateway calls it from a
detached task whose own comment says callers "MUST NOT depend on the envelope
being durably stored" (`a2a-gateway/src/runtime/audit_publish.rs:17-23`), and
every dispatch outcome, a Tier-1 denial included, goes through it
(`runtime/tier1_denial.rs:65`, `runtime/dispatch.rs`). The call is a no-op unless
`A2A_GATEWAY_AUDIT_PUBLISH` is set, which defaults to off "so a deployment
doesn't start publishing to a stream operators haven't provisioned"
(`runtime/env.rs:36-40`). No stream is provisioned: the repository holds no
JetStream configuration for audit subjects. A refused request, the one record an
investigation cannot do without, is therefore the record most likely to vanish: a
NATS hiccup, a restart or an unprovisioned stream drops it silently.

**A second ingress builder exists and nothing calls it.**
`a2a-gateway/src/audit_ingress.rs` defines `A2aIngressAudit` and
`ingress_audit_subject`, whose subject is
`{prefix}.a2a.audit.{outcome}.ingress.{skill}` (`:3`, `:238-248`). Only
`lib.rs:40` mentions the module. [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)
(draft) already records that this grammar has no version token and grows with the
skill set, and says to correct it "when the ingress audit path is wired rather
than ahead of it". This is that decision.

**The fields an audit record would need are spread across vocabularies that do
not meet.** [ADR#0078](./0078-api-key-rotation-grace-and-audit-set.md) defines
`AuditActor` with unattributed, API key, human session and break-glass arms
(`proto/trogonai/platform/api_keys/audit/v1/audit_actor.proto`) and an
`ApiKeyAuditFact`; key custody has its own `KeyOperationAuditRecord`
(`proto/trogonai/platform/key_custody/audit/v1alpha1/audit_record.proto`) under
[ADR#0030](./0030-customer-controlled-key-backend-routing.md). Both are shaped
well, both state that nothing secret can be represented in them, and neither has
Rust bindings: `proto/trogonai/platform` is in the `exclude_paths` of
`buf.gen.yaml`, and `trogonai-proto` has only the `gateway`, `schedules`,
`grpc-nats-micro` and `decider` features
(`rsworkspace/crates/platform/trogonai-proto/Cargo.toml:11-22`). Policy
decisions are attributed by a `PolicyRuleRef`, the kind, subject and stream
revision a decision consulted
([ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
Decision 7), and no record type carries one. The actor of a domain event is not
recorded at all today (the event metadata documentation lists it as not
recorded), and [ADR#0069](./0069-event-envelope-records-issuer-and-times.md)
(draft) proposes to add it.

**The promises already made have no contract behind them.** ADR 0085 (agent
egress proxy, proposed) Decision 5 says every egress decision emits one audit
event carrying the decision id, tenant, agent, Session, destination, the
`PolicyRuleRef` list and projection revision, the TLS mode, the fault code, and
the binding and credential ids when a credential was injected; that no event
carries a header value, a body or a credential; that the proxy refuses new
connections with `AUDIT_UNAVAILABLE` when it cannot emit an event; and that a
deny stands when its event cannot be published. It defers "the audit envelope and
its stream" to a later ADR. ADR 0084 (enforcement posture, proposed) makes every
enforcing point refuse to start unenforced and records `would_deny` for DryRun,
but says nothing about a point that is enforcing and cannot record what it
enforced. [ADR#0023](./0023-secret-management-and-key-custody-direction.md) makes
"correct behavior when audit sinks fail" a gate of adopting the secrets service
and decides nothing about it.

**What this decision does not have a basis for.** There is no content-inspection
stage in the repository: Tier-3 redaction rewrites responses and no code or ADR
defines a guardrail verdict or a "not inspected" state. Nothing classifies a
target as read, write or high risk. There is no consent flow whose completion
step could go unaudited; the credential aggregate's transitions are the ten
events of `proto/trogonai/gateway/credentials/v1/events.proto`. This record does
not invent them.

## Decision

A **record** is one `AuditRecord`. A **writer** is one process instance that
publishes records. A **point** is a process that enforces or releases something,
such as a policy enforcement point or the secrets service, and a **layer** is
one of its policy layers. ADR 0084 and ADR 0085 are proposed and not yet
merged, so they are cited by number only and their terms are used by name.

### 1. One envelope, `trogonai.platform.audit.v1.AuditRecord`, with typed detail

The package sits beside `platform.api_keys.audit.v1` and
`platform.key_custody.audit.v1alpha1` under `proto/trogonai/platform/audit/v1/`,
one type per file under [ADR#0009](./0009-protocol-buffers-wire-contracts.md).
In prose, because field numbers and validation belong to the proto review:

| Field | Meaning |
| --- | --- |
| `record_id` | A UUIDv7, also the decision identifier ADR 0085 names and the `Nats-Msg-Id` of Decision 3. The workspace already enables `v7`. |
| `class` | `MUST` or `BEST_EFFORT`, set by the table in Decision 3, never chosen by a caller. |
| `occurred_at`, `recorded_at` | When the writer observed the event, and when it handed the record to the sink. Their names follow [ADR#0069](./0069-event-envelope-records-issuer-and-times.md) if it lands first. |
| `tenant`, `project` | Required. The tenant is what the subject carries ([ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) Decision 3) and the project is the anchor of [ADR#0046](./0046-project-anchored-resource-hierarchy.md). |
| `actor` | `AuditActor`, below. |
| `act_chain` | The delegation chain behind the actor, bounded at 16 entries, which is `MAX_CHAIN_DEPTH` in `trogon-aauth-verify`. `flatten_act_chain` already produces it and no consumer writes it. |
| `trace_id`, `request_id` | The W3C trace id, for correlation with traces, and the id the surface uses for the request. |
| `attribution` | Present on decisions and credential records: the decision (allow or deny), the `EnforcementMode` (`ENFORCE` or `DRY_RUN`, the values of ADR 0084), `would_deny`, a closed `reason_code` owned by the surface, the rule ids that fired, the `PolicyRuleRef` list, and the policy source and projection revision. |
| `integrity` | Writer id, per-writer sequence, previous hash and record hash, filled by the sink (Decision 6). |
| `detail` | A `oneof` of `GatewayDecision`, `CredentialUse` and `Change`. |

**The actor block is the ADR 0078 shape, extended once.** The new package defines
its own `AuditActor` with the four arms of ADR 0078 and one more, `agent`, which
holds the platform's `AgentId` (the identity under
[ADR#0017](./0017-aauth-agent-authentication.md) and the mTLS identity of ADR
0085). The api-keys package keeps its message until its Rust bindings exist, so
no existing proto moves. There is deliberately no `system` or `component` arm.
The comment on `audit_actor.proto`, written for
[ADR#0078](./0078-api-key-rotation-grace-and-audit-set.md), explains that a
synthetic "system" actor "would make the audit log claim an authentication that
never happened", and that reasoning holds here: the process
that wrote a record is named by `integrity.writer_id`, which is a statement about
the writer and not about who caused the act. A runtime point sets `actor` from the
principal it authenticated, derived from the subject inside the bus
([ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
Decision 1) or proved at the edge, never from a header. A projected record
(Decision 7) carries the actor of the source event when the event envelope has
one and `unattributed` when it does not, which is today's state; the projector
never infers one.

**`GatewayDecision`** carries the surface (A2A ingress, egress), the method or
destination as a registry-style target, request and response sizes, latency, the
TLS mode (passthrough or intercepted) for egress, and a list of layer verdicts.
The A2A adapter maps `AuditEnvelopeFields` into it: `rules_fired`, the SpiceDB
consistency token, and the Tier-1 and Tier-3 decisions become layer verdicts, and
`caller_id` becomes the actor. `rewrites` and `stream_consumer` are A2A routing
detail and are not carried.

**`CredentialUse`** carries the kind (release or inject), the credential
resource name, the credential binding id, the destination, and the `record_id` of
the decision it served. It carries no value, placeholder text or digest.

**`Change`** carries the kind (policy or credential), the resource name, the
event type, and the source stream, stream sequence and event id. It is a pointer
to a decider event, not a copy of one (Decision 7).

The A2A `AuditEnvelope` stays the source type inside `a2a-nats` and the gateway,
and an adapter builds records from it. The `ApiKeyAuditFact` and
`KeyOperationAuditRecord` stay as they are and may become `detail` arms when their
bindings exist; this record leaves room and does not decide it.

**An `UNINSPECTED` marker is deferred.** A marker for a request that was allowed
without content inspection needs an inspection stage and a rule for when one must
run. Neither exists, and a marker with no stage behind it would be a field no
writer could fill truthfully. It becomes an `attribution` field when a guardrail
contract exists.

### 2. Subjects follow ADR 0055's operational profile, and the stream refuses writes when full

```text
{prefix}.v1.audit.decision.{project}.{allow|deny|would_deny}
{prefix}.v1.audit.credential.{project}.{release|inject}
{prefix}.v1.audit.change.{project}.{policy|credential}
{prefix}.v1.audit.integrity.{component}.{start|chain_reset}
{prefix}.v1.audit.checkpoint.{component}.signed
```

This is the `<operational-root>.<bounded-entity>.<fixed-terminal>` form of
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md), with the audit kind
and the project as entity tokens and a closed terminal vocabulary. The terminals
describe what happened to the act: `would_deny` is a DryRun refusal that was
forwarded. Per-request values (agent, tool, host, method) are payload fields,
because an entity token must be long lived and the project already scopes
credential subjects ([ADR#0046](./0046-project-anchored-resource-hierarchy.md)).
`component` names the kind of writer, such as the A2A gateway or the egress
proxy, not an instance, which would churn with every restart.

ADR 0055 defines the prefix as `[{tenant}.]{protocol}` and has no protocol for a
family that spans them, since the A2A gateway, the egress proxy and the secrets
service write into it. The prefix here is the fixed token `platform`, with the
tenant leading inside `PLATFORM` as in ADR 0081 Decision 3. That keeps the
family disjoint from the legacy A2A emitter's
`a2a.v1.audit.{agent_id}.{ok|err|lifecycle}`, and a tenant account can import
the records whose leading key is its own. The read path through that import is
not designed here. ADR 0055 is still a draft, so this is a request that it note
`platform` as a prefix token and `audit` as an operational root with these kinds.
`ingress_audit_subject` is deleted, and the live A2A emitter stops publishing
decisions when the adapter replaces it; task lifecycle records are not a decision
and keep their subject until a later record maps them.

Two streams hold the records, as direction and not as a provisioning script.
`AUDIT` takes the first four kinds. It uses file storage, three replicas in
production and one in the dev compose, limits retention, `discard: new` so a full
stream refuses a write and does not drop the oldest record, `deny_delete` and
`deny_purge`, a two-minute duplicate window, and no `max_age`. It is encrypted at
rest by [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md)
Decision 6 like every store. `AUDIT_CHECKPOINT` takes the checkpoint kind with the
same safety flags. Retention follows the explicit-watermark habit of
[ADR#0029](./0029-decider-retention-and-truncation-watermark.md): nothing
expires on a timer, and how an operator truncates after archiving, and the window
itself, are left open (Consequences). Only the platform workloads that write
records hold publish permission on `platform.v1.audit.>`, granted through the
admission topology of ADR 0081 Decision 4. Agents and tenant callers hold none.

### 3. A record is must-audit unless its surface declares it best-effort

| Class | Records |
| --- | --- |
| `MUST` | Every deny in `ENFORCE` mode, at any layer. Every credential release, every injection and every refused release. Every credential aggregate transition, failures included (write, rotation and destroy requested, completed or failed, activation, revocation). Every policy change: bindings, ceilings and holds under ADR 0071. Every record whose actor is a break-glass actor. Writer start and chain reset. Every allow on a surface that has not declared itself read-only. |
| `BEST_EFFORT` | An allow on a surface that declares the method read-only. Every DryRun record, `would_deny` included. |

The default is `MUST`, so a surface that says nothing is audited durably. A
surface may declare a method read-only in its own contract only when the method
has no effect on the target. The egress proxy declares none: a passthrough
tunnel is opaque, so the proxy cannot tell a read from a write, and every egress
allow is must-audit, which is what ADR 0085 Decision 5 requires. For the A2A
ingress the declaration is a small set of methods in the gateway's contract and
anything outside it is must-audit. The class is fixed by kind, terminal and that
declaration, and the `class` field records the outcome; a writer never decides
it per call. A "high-risk" or "first-seen" allow is not a class here,
because nothing in the repository tags a target that way.

A `MUST` record is published to JetStream with `Nats-Msg-Id` set to `record_id`
and the writer waits for the `PubAck`. A timeout is retried with the same
message id, so a lost ack cannot store the record twice, and a record that is
not acknowledged is treated as not stored. Provenance travels in headers, never
in payload fields ([ADR#0039](./0039-self-authenticating-event-provenance.md)),
so the writer id and sequence ride as `Trogon-Audit-Writer` and
`Trogon-Audit-Seq`. A `BEST_EFFORT` record goes through a bounded in-memory queue
with an asynchronous publish. A full queue drops the newest record and counts the
drop by class and reason. Dropping never refuses a request, and no `MUST` record
is ever sampled.

DryRun records are best-effort, and that has a cost: the counts that
ADR 0084 Decision 3 uses to decide when a default is safe to flip are only as
complete as the drop counter says. An operator reading `would_deny` totals reads
the drop counter beside them. Credential and change records are `MUST` in every
mode, because a release or a policy change has its effect whether or not a layer
is in DryRun.

### 4. A point that cannot durably write a must-audit record refuses the action

- **Write before the effect.** For an allow whose effect is a forward, a
  credential release or an injection, the record is acknowledged before the
  effect. If it is not, the action is refused with the surface's operational
  unavailability code: `AUDIT_UNAVAILABLE` with 503 and `Retry-After` on HTTP, as
  in ADR 0085, and the equivalent server error on A2A and other bindings. It is
  an operational fault and never reported as a security denial, so "you may not"
  and "we cannot record this right now" stay different answers
  ([ADR#0079](./0079-external-secret-sources.md) Decision 5: denied,
  unavailable and throttled stay distinct). The refused attempt has no record
  because none could be written; it increments a counter and writes one
  structured local log line.
- **A deny stands.** A deny takes effect whether or not its record is written.
  The record goes to a bounded retry buffer and is retried with the same message
  id. When the buffer is full the writer is audit-degraded: it refuses every
  action that would need a `MUST` record before an effect, until the buffer
  drains, and keeps denying what it denies. A deny that cannot be buffered is
  dropped and counted in a counter that pages. This is the one place a `MUST`
  record can be lost, and the limit is stated in Consequences.
- **A full stream is the same refusal.** `discard: new` returns a publish error
  that the sink maps to a distinct reason, so a full stream and an unreachable one
  page differently and refuse identically.
- **On by default.** When any layer of a point is in `ENFORCE`, audit publishing
  is on and cannot be turned off. `A2A_GATEWAY_AUDIT_PUBLISH` stops being a
  default-off switch: with an enforcing layer, setting it false is a startup error
  of the same typed kind as ADR 0084's missing-policy refusal, and a point with
  no enforcing layer, which ADR 0084 allows only behind its development escape
  hatch, keeps it opt-in. Audit configuration that is absent or malformed stops
  the process at start. A stream that is configured and unreachable at runtime
  does not crash the process: the point serves what needs no `MUST` record
  before an effect, such as denies, refuses the rest, and fails readiness.
- **DryRun.** In a DryRun layer a failed decision record changes no verdict,
  because nothing is enforced, and increments the drop counter. Credential and
  change records follow the rules above in every mode.

An audit outage is therefore an availability dependency of the enforcing points,
and the stream's replication is how that dependency is made small. No bypass is
defined here. Records state decisions, releases and changes, not completed
effects: an ack lost on a record whose action was then refused can leave a record
for an action that did not happen, and the point's outcome, when it matters, is a
separate best-effort record.

This satisfies each promise of ADR 0085 Decision 5. The decision record carries
the identifier (`record_id`), tenant, agent (`actor`), Session (a field of
`GatewayDecision`, from the certificate), destination, the `PolicyRuleRef` list
and projection revision, the TLS mode and the fault code. The binding and
credential identifiers sit on the `CredentialUse` record of each injection, which
names the decision it served, because injection happens per request inside a
tunnel the decision admitted. Nothing carries a header value, a body or a
credential (Decision 5). `AUDIT_UNAVAILABLE` and "a deny stands" are the first
two bullets above.

### 5. Records hold no secret, token, prompt, tool argument, header value or body

The rule is structural, as in ADR 0078 Decision 8 and ADR 0030: no field of any
record has a type that could hold such a value, and the record types implement
`Debug` without the sensitive newtypes' contents. Concretely:

- there is no free-text field a caller fills. `reason_code` is a closed
  vocabulary owned by the surface, and the A2A `AuditOutcome::Err { code, message }`
  maps to its code and drops its message, which can echo content;
- targets are names (resource names, hosts, method names), never payloads;
- sizes replace content: request and response byte counts;
- a digest of caller-supplied content is not recorded. An unkeyed digest of
  low-entropy content is a confirmation oracle for a reader who can guess the
  content, the same flaw [ADR#0083](./0083-sealed-content-envelope.md) names in
  digest-named claim-check blobs. The A2A `params_fingerprint` is
  `Sha256::digest` of the raw params (`envelope.rs`), so the adapter does not map
  it. A digest appears only for identifiers that are high-entropy by
  construction;
- a credential appears by resource name and binding id, never by value,
  placeholder text or digest.

Opt-in redacted capture of content is a different feature with its own decision
and is not available as a setting on a record. A test per writer drives it with
canary secrets, tokens, prompts and header values and asserts that none reaches a
serialized record, a `Debug` rendering or a log line, in the way the API-key tests
already assert no plaintext in logs and traces.

### 6. Integrity is a per-writer hash chain with signed checkpoints, and archive is deferred

Each writer keeps one chain. `integrity` holds the writer id, a gap-free
sequence, the previous record's hash and the record's SHA-256 over its
deterministic encoding with the hash field empty. One task per writer assigns
sequence numbers and does not move past a `MUST` record that has not been
acknowledged, so the stored chain has no gap and a blocked head is the
audit-degraded state of Decision 4. A restarted writer resumes from its last
stored record, or starts a new chain and writes a `chain_reset` record that says
so.

Periodically each writer publishes a signed checkpoint to `AUDIT_CHECKPOINT`: the
writer id, its head sequence and hash, the `AUDIT` stream sequence at that
moment, the time and the signer's key id. The signature is Ed25519, the default
suite of [ADR#0038](./0038-agent-identity-crypto-suite.md), under a platform key
held under [ADR#0023](./0023-secret-management-and-key-custody-direction.md) and
[ADR#0033](./0033-key-custody-product-model.md) custody. A verifier then detects
an edited, reordered, duplicated or deleted record in the middle of a chain, and
a truncated tail at the next checkpoint. It does not detect an attacker who can
write both streams and use the signing key, and it makes no claim of global order
across writers: the chains are independent, which is what lets writers run
without a shared lock. Cutting the chain format and cadence into a proto and a
verifier is implementation work. Archiving to object storage with object lock
(WORM) is deferred. Without it the record of last resort is the two streams under
`deny_delete` and `deny_purge`, which stop an application-level deletion and not
an operator of the NATS server.

### 7. Runtime points write decisions inline, and a projector writes changes

Decisions, credential releases and injections are not domain events: the egress
proxy has no decider and a gateway refusal changes no aggregate. Their writer is
the point that made them, inline, under Decision 4.

Policy and credential changes already are events. The credential aggregate's ten
events are append-only facts, and bindings under ADR 0071 will be the same
([ADR#0045](./0045-event-sourced-service-module-layout.md),
[ADR#0057](./0057-decider-command-nats-binding.md)). A hand-written audit record
beside them would be a second source of truth that could disagree. So a
[projection](../glossary/projection), built on the workspace's `Projector`
catch-up primitive, reads the decider streams and writes `Change` records that
point at the event: stream, sequence, event id and event type. A credential
stream is sealed ([ADR#0083](./0083-sealed-content-envelope.md) Decision 1) but
`Nats-Msg-Id` and `Trogon-Event-Type` stay plaintext headers, so the projector
needs no opener and sees no payload. The projector publishes `MUST` records with
acknowledgement and does not advance its checkpoint past one it has not stored.
It records the source sequence so a gap in what it saw is detectable.

For a change, the durable record is the event, appended before any effect. The
audit record follows. Fail closed therefore means something different here: the
command is committed by the decider, and the audit projection lags and alarms.
That is sound only while the decider stream cannot be truncated past what the
projector has stored, and ADR 0029's watermark does not know about the projector
yet. Making the projector's checkpoint an input to the
[retention watermark](../glossary/retention-watermark) is a requirement on the
implementation of the projector (Consequences).

### 8. Export is OTLP logs and JSON Lines, and there is no SIEM schema

A JetStream consumer per export target reads `AUDIT` and writes OpenTelemetry
logs, correlated by `trace_id`, and a JSON Lines sink adapter for object storage,
Kafka or HTTP, using the protobuf JSON mapping of the published proto. Delivery
is at least once and `record_id` is the key to deduplicate on. Export lag and
drop are metrics. The stream is the record, so a failing exporter never affects a
writer or an enforcing point. The platform does not define a schema for a
particular security information product; consumers map the published proto and
its JSON form themselves.

## Alternatives Considered

**Keep per-domain audit facts and unify at export.** Rejected. API-key facts,
key-operation records and gateway decisions would each carry their own actor,
timestamps, class and integrity fields, and a reader would ask three streams the
question "what did this agent do". The two existing facts stay as detail arms
when they are bound.

**Keep fire-and-forget core NATS publishing.** Rejected. It is today's behavior,
and it loses exactly the records an investigation needs: denials, credential
releases and changes.

**Acknowledge every record.** Rejected. It puts a JetStream round trip and an
audit outage on every read allow. ADR 0078 Decision 6 already declined a
per-request audit fact for successful verification and chose a metric, and a
per-request durable write on every read allow would repeat that cost at request
volume. A declared read-only class keeps the cost where nothing needs it.

**Fail open with an alarm when the audit stream is down.** Rejected. It turns an
outage, or an attacker who can cause one, into a window in which credentials are
released and requests forwarded with no record, and it contradicts both the
enforcement posture of ADR 0084 and the refusal ADR 0085 promises.

**Write to a local disk spool and ship asynchronously.** Rejected for now. A
spool is a second durability store in every process, with its own recovery,
rotation and tamper story, and it makes an audit outage invisible to the request
until the disk fills. It is revisited if the acknowledgement latency of
Decision 3 proves too high.

**Sign every record.** Rejected. A signature per record detects an edit and not a
deletion, and pays a signature on the request path. Signing a chain head at
checkpoint granularity covers deletion and reorder and signs once per batch.

**One global hash chain.** Rejected. It serializes every writer through one lock
or one sequencer and makes that sequencer a single point of failure for every
enforcing point. Per-writer chains with a checkpoint that binds each head keep
writers independent.

**Derive every record from decider events.** Rejected for decisions and releases,
which are not state changes of any aggregate and have no decider to append to.
Accepted for changes, where the event is the truth (Decision 7).

**Carry audit in event provenance headers.** Rejected.
[ADR#0039](./0039-self-authenticating-event-provenance.md) authenticates who
authored a command. It does not chain records, name a decision or protect a
stream from deletion, and system-derived records carry no provenance header.

**Define an export schema for a security information product.** Rejected. It
couples the published contract to one consumer's field names and forces the
platform to track that product. A documented proto and an OTLP mapping serve
every consumer.

## Consequences

- A new platform crate, `rsworkspace/crates/platform/trogon-audit`, holds the
  record builders, the subject builder, the sink with its `MUST` and `BEST_EFFORT`
  classes, the redaction guarantee and the chain writer, with an in-memory sink
  that can inject failures. The writers depend on it and none builds a record by
  hand.
- `buf.gen.yaml` stops excluding `proto/trogonai/platform`, or excludes only the
  sub-packages that stay out, and `trogonai-proto` gains an `audit` feature. That
  is the promotion moment its comment ties to
  [ADR#0064](./0064-schema-constraints-as-documentation.md), so the validation
  annotations on the new protos decide there whether they become enforced.
- The A2A gateway is rewired to the sink: decisions are acknowledged before the
  forward and denies are buffered. `audit_ingress.rs` is deleted, the live
  emitter stops publishing decisions, and `A2A_GATEWAY_AUDIT_PUBLISH` changes
  meaning as Decision 4 says. Tests that start a gateway with an enforcing layer
  need a stream or the sink's fake.
- Every enforcing request now pays a JetStream acknowledgement, bounded by a short
  timeout, and an unreachable stream refuses must-audit actions. The dev compose
  provisions both streams, and production provisioning sets replicas and alerts.
- A full `AUDIT` stream stops every enforcing point, because `discard: new` is the
  fail-closed choice. Capacity alerts must fire well before that, and the
  retention window, the archive and the truncation procedure under `deny_purge`
  are decided before a production stream fills.
- The projector must make its checkpoint an input to the decider retention
  watermark before any decider stream it reads is purged, and credential and
  policy changes reach the audit stream with the projector's lag.
- Records are readable by `PLATFORM` workloads and are not sealed. They name
  credentials, destinations and agents without values, which is an inventory of
  the kind ADR 0083 seals in credential records. Whether a customer managed
  tenant's audit stream is declared sensitive under that decision is not decided
  here.
- The revocation target of [ADR#0049](./0049-revocation-latency-target.md) stays a
  metric. A revocation is a `Change` record and a later deny names the
  `PolicyRuleRef` revision it evaluated, so the interval is reconstructable, but
  no field records the first deny after a revoke.
- Tests follow [ADR#0010](./0010-testcontainers-for-infrastructure-tests.md):
  chain, class and redaction logic run on the in-memory sink, and only the
  acknowledgement, duplicate-window and full-stream behavior run against a real
  JetStream container.
- ADR 0055 is asked to note the `platform` prefix token and the audit kinds, and
  ADR 0085 Decision 5 is satisfied as Decision 4 states, with the binding and
  credential ids on the injection record. [ADR#0078](./0078-api-key-rotation-grace-and-audit-set.md)
  gains a note that `AuditActor` now has a shared definition. The open item in
  ADR 0023 about audit sink failure is answered by Decision 4.
- Stated limits: a `MUST` deny lost when its retry buffer overflows or the
  process exits first; a record for an action that was refused because an
  acknowledgement was lost; no global ordering across writers; an attacker who
  controls the NATS server or holds the checkpoint signing key; no WORM archive;
  no `UNINSPECTED` marker, read-only classification beyond what a surface
  declares, read path for tenants, or opt-in content capture.

## References

- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0010: Unit Tests First, Testcontainers Only When Necessary](./0010-testcontainers-for-infrastructure-tests.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0029: Snapshot-Derived Retention Watermark for Decider Streams](./0029-decider-retention-and-truncation-watermark.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](./0030-customer-controlled-key-backend-routing.md)
- [ADR#0033: Key Custody Product Model](./0033-key-custody-product-model.md)
- [ADR#0038: Agent Identity Cryptographic Suite and Crypto-Agility](./0038-agent-identity-crypto-suite.md)
- [ADR#0039: Self-Authenticating Event Provenance](./0039-self-authenticating-event-provenance.md)
- [ADR#0045: Aggregate-Oriented Module Layout for Event-Sourced Services](./0045-event-sourced-service-module-layout.md)
- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)
- [ADR#0057: Decider Command NATS Binding](./0057-decider-command-nats-binding.md)
- [ADR#0064: Invariants Are Declared in the Schema Before Anything Enforces Them](./0064-schema-constraints-as-documentation.md)
- [ADR#0069: Every Event Envelope Records Who Caused It and When Each Step Happened](./0069-event-envelope-records-issuer-and-times.md)
- [ADR#0071: What an Agent May Reach, How Far It May Go, and Whether It May Run at All Are Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
- [ADR#0078: API Key Rotation Grace and the First-Release Audit Set](./0078-api-key-rotation-grace-and-audit-set.md)
- [ADR#0079: External Secret Sources Resolve Behind the Secrets Service](./0079-external-secret-sources.md)
- [ADR#0081: Every NATS Connection Uses TLS Unless Explicitly Disabled, and Clients Enter Operator-Mode Accounts Only Through Platform Credentials or the Auth Callout](./0081-nats-tls-by-default-and-operator-mode-admission.md)
- [ADR#0082: Inside the Bus the Principal Is Derived from the Subject, and Proof of Possession Is Enforced Only at the External Edge](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
- [ADR#0083: Credential Material, Claim-Check Blobs, and Sensitive Tenant Streams Under a Customer Managed Key Are Sealed in an Owned AEAD Envelope, and Nothing Else Is](./0083-sealed-content-envelope.md)
- ADR 0084: Every Policy Enforcement Point Enforces by Default and Refuses to Start Unenforced (proposed, not yet merged)
- ADR 0085: Agent Egress Proxy (proposed, not yet merged)
