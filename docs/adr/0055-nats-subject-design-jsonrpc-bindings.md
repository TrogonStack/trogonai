---
number: "0055"
slug: nats-subject-design-jsonrpc-bindings
status: draft
date: 2026-08-08
---

# ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings

## Context

The repository routes JSON-RPC 2.0 protocols — MCP, ACP, and A2A — over the NATS
backbone ([ADR#0003](./0003-ai-protocol-transport-taxonomy.md)).
[ADR#0054](./0054-nats-protocol-binding-documentation.md) selects AsyncAPI as the
artifact that documents those bindings. Neither states **how the subjects
themselves are designed**.

Each adapter has so far grown its own subject grammar with independent choices:
connection-keyed versus session-keyed routing, presence or absence of a version
token, whether request identifiers appear in subjects, and whether JetStream is
used at all. That divergence is a problem: subjects are the part of the system
that NATS routing, authorization, scaling, and JetStream all depend on, and they
should follow one standard rather than per-author convention.

This ADR defines that standard for the **message-oriented** NATS subjects this
repository designs — JSON-RPC protocol bindings and operational subjects, which
share the same routing cache, subscription trie, JetStream listings, prefix authz,
and `subject_transform` constraints. The JSON-RPC binding profile is the primary
worked form; operational roots follow the same cardinality and versioning rules
under a thinner profile. Protobuf micro RPC keeps its own descriptor-derived
subject rule; see Scope.

It is **normative and prescriptive**. Existing adapter code is evidence of what
has been tried, not the specification; where current behavior conflicts with this
ADR, the code migrates. In particular, the fact that one protocol is currently
core-only or another currently encodes a request id in a subject does not make
either pattern correct.

The design is constrained by how NATS actually behaves:

- Subjects are matched left to right with `*` (one token) and `>` (trailing).
  Wildcards exist only in subscriptions, never in published subjects.
- Authorization is expressed as subject-prefix allow/deny rules.
- Horizontal scaling uses queue groups (core) or durable consumers (JetStream)
  on a shared subject.
- JetStream streams and consumers select by subject filter and can apply
  `subject_transform` and partitioning.
- The subject routing cache (1,024 entries, drained to 512) loses effectiveness
  against unique-per-message subjects; the subscription trie and JetStream
  subject-detail listings (capped at 100,000 per page) grow with cardinality.
- Subjects beyond 32 tokens trigger heap allocation; stay under 16 tokens and
  256 characters. Subjects are case-sensitive.
- `Nats-Msg-Id` is reserved for JetStream deduplication.

## Decision

### Scope

This standard governs the subjects this repository designs for **message-oriented
bindings**: the JSON-RPC protocol profiles (MCP, ACP, A2A) and the operational
profile below.

It does **not** govern protobuf RPC endpoints, whose subject is derived
mechanically from the service descriptor by
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md)
(`<group>.<EndpointName>`, the endpoint name mirroring the gRPC method name).
That derivation is owned by the micro binding and deliberately does not carry a
`v{major}` token or lower_snake terminals, because a NATS micro service versions
through its service descriptor rather than its subject. Cardinality (below) still
applies: a micro endpoint subject is bounded by the service and method set, never
by request.

### Canonical grammar

Every subject is built from these elements, in this order:

```text
{prefix}.v{major}.<routing-segment>.{terminal...}
```

- `prefix` — the configured subject namespace: a dotted NATS token (max 128
  bytes, default = the protocol name). It carries `[{tenant}.]{protocol}` — the
  protocol identity (`mcp`, `acp`, `a2a`) optionally preceded by a tenant
  namespace when several tenants share one NATS account (see Authorization and
  multitenancy; e.g. `mcp`, `acme.mcp`). The
  protocol is NOT a separate token; it is the protocol-identifying suffix of the
  prefix that each deployment configures and each parser strips. The word
  `protocol` is always written in full to avoid confusion with Protocol Buffers.
- `v{major}` — the **binding** version: the first structural token after the
  prefix, owned by the binding (not the operator-configured prefix). It versions
  ONLY the subject/routing contract, never the payload — see Versioning Posture.
  Established from day one.
- `<routing-segment>` — one or more bounded-entity tokens followed by a role
  token where the profile requires one (see Routing segment).
- `{terminal...}` — for JSON-RPC bindings, a projection of the method under the
  binding's method-to-terminal mapping (see below), ALWAYS last so a single
  subtree subscription plus in-process dispatch covers every method. For
  operational subjects, a small fixed terminal vocabulary (not a growing method
  set) — see Operational profile.

Two ordering invariants are required; the rest is profile-defined:

1. The configured `prefix` is leftmost and is immediately followed by `v{major}`.
2. The subtree a responder owns MUST be a contiguous left-prefix ending before
   `{terminal...}`, so the responder can cover it with one `...>` subscription.

### Method-to-terminal mapping

The terminal is a **projection** of the JSON-RPC method, not a copy of it. The
naive rule "render `/` as `.`" is insufficient: it produces tokens that violate
the casing rule under Limits, and it has no answer for methods the binding does
not know at build time.

Each JSON-RPC binding MUST define a mapping between its method set and its
subject terminals, and MUST publish it — [ADR#0054](./0054-nats-protocol-binding-documentation.md)
records it in the binding's AsyncAPI document. The mapping MUST be:

1. **Total and bidirectional.** Every method projects to exactly one terminal,
   and every terminal resolves back to exactly one method — including methods
   outside the known set. The routing segment MAY serve as context for the
   reverse direction (ACP resolves the terminal `prompt` to `session/prompt`
   because the subject already carries `session.{session_id}`).
2. **Token-safe.** Terminals obey Limits: lower_snake, no wildcard characters,
   inside the token and byte budget. MCP folds case for this reason
   (`logging/setLevel` → `logging.set_level`, `sampling/createMessage` →
   `sampling.create_message`).
3. **Stable.** Changing the mapping changes the subject contract and is governed
   by `v{major}`.

**Escape encoding.** Totality requires an encoding for methods outside the known
set. That encoding MUST be distinguishable by a reserved leading token (MCP uses
`custom.{base64url}`) and is exempt from the lower_snake rule, since it must
round-trip arbitrary method strings. It remains bound by the wildcard, token-
count, and byte limits.

**The projection is one-way authority.** The mapping governs the subject only.
The body always carries the protocol's own method string verbatim
([ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md)); a binding MUST NOT
substitute the projected terminal for the real method in the body. "Subject and
body agree" means the subject terminal is the projection of the body method
under this mapping, not that the two strings are equal.

### Routing segment

The routing segment is not a closed two-value enum. It is a rule:

> One or more **bounded-entity** tokens (long-lived: peer, session, task,
> gateway hop, catalog entry, …), then — when more than one side owns an inbound
> subject under the same entity — a single stable **role** token naming the
> owning side.

`role` is the authorization and subscription split point. It MUST be a single,
stable token, and it is **required only where the split exists**. MCP and ACP
need it because both sides receive requests (`server`/`client`, `agent`/
`client`). A subtree with exactly one receiving side has no split to name and
takes no role token; the entity-type token that leads it (`agents`, `tasks`,
`gateway`) is a routing root, not a role.

Worked examples (not an exhaustive enum):

- **Connection-scoped, role-split** (e.g. MCP): `<routing-segment> =
  {role}.{peer_id}`, because both sides receive requests.
  - `mcp.v1.server.{server_id}.tools.call`
  - `mcp.v1.client.{client_id}.sampling.create_message`
  - `acme.mcp.v1.server.{server_id}.tools.call` (tenant `acme` via dotted prefix)
  - responder subscribes `mcp.v1.server.{server_id}.>`
- **Connection-scoped, single-sided** (e.g. the A2A agent and gateway surfaces):
  `<routing-segment> = {root}.{peer_id}` with no role token, because only the
  agent side receives. `agents` and `gateway` are separate routing roots (see
  Multiple routing roots), not two roles on one root.
  - `a2a.v1.agents.{agent_id}.message.send`
  - `a2a.v1.gateway.{agent_id}.message.send` (the ingress hop)
- **Session-scoped** (e.g. ACP):
  `<routing-segment> = session.{session_id}.{role}`, with a reserved `global`
  segment for operations that precede any session id.
  - `acp.v1.global.agent.initialize`
  - `acp.v1.session.{session_id}.agent.prompt`
  - `acp.v1.session.{session_id}.client.terminal.create`
  - responder subscribes `acp.v1.session.*.agent.>` (or one session's
    `...{id}.agent.>`)
- **Task-scoped** (e.g. A2A task events):
  `<routing-segment> = tasks.{task_id}` — a bounded entity without a role token,
  because the stream is fan-in of events for that task, not a role-split RPC
  surface.
  - `a2a.v1.tasks.{task_id}.events`
  - consumers filter `a2a.v1.tasks.{task_id}.events` (or `a2a.v1.tasks.*.events`
    for a gateway-wide durable consumer)

### Multiple routing roots

One protocol MAY span multiple routing roots under the same configured prefix
(A2A already spans `agents.*`, `tasks.*`, and `gateway.*`). Each root is its own
contiguous left-prefix subtree. The "one contiguous left-prefix subtree per
responder" invariant applies **per root**, not across the whole protocol
namespace: a responder that owns `a2a.v1.agents.{agent_id}.>` is not required to
also own `a2a.v1.tasks.>`.

### Cardinality — the central rule

Subject token cardinality MUST be bounded by **long-lived entities**: tenant,
binding version, role, logical peer, connection, session, task, caller. Subjects
MUST NOT contain **per-request or per-message** identifiers — JSON-RPC `id`,
request UUIDs, or correlation ids. Per-request routing uses the reply inbox and
headers, never the subject. Unique-per-message subjects defeat the routing cache,
inflate the subscription trie, and exhaust JetStream subject listings.

### Correlation and headers

- Request/response correlation is a **transport** concern, per
  [ADR#0011](./0011-jsonrpc-over-nats-binding.md) / upon acceptance
  [ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md): the NATS reply inbox
  for core request/reply, and a **correlation token** in a header plus a response
  consumer across a durable hop where the reply inbox is not available.
  Correlation MUST NOT use `Nats-Msg-Id`.
- The JSON-RPC `id` is an **application**-level token that travels in the
  canonical body ([ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md)) and is
  projected to the non-authoritative `Jsonrpc-Id` header. The response `id` MUST
  equal the request `id`.
- **Which token correlates a durable hop** depends on who mints the `id`:
  - Where the **transport itself mints** the JSON-RPC `id` and the response
    subject's entity scope bounds its uniqueness, the binding MAY correlate on
    the `id` (read from `Jsonrpc-Id`, authoritative value in the body). ACP does
    this: the bridge mints a UUID v7 per request, and the response subject is
    already scoped to one session.
  - Where the `id` is **peer-supplied**, the binding MUST carry a distinct
    transport-minted `Trogon-Req-Id` and correlate on that. A2A does this: an A2A
    caller chooses its own `id`, so it is neither unique across callers nor
    trustworthy for demux.

  A binding MUST NOT correlate on a peer-supplied `id`. Emitting `Trogon-Req-Id`
  alongside a transport-minted `id` that already carries the same value is
  duplication and SHOULD be avoided.

  `X-Req-Id` is the name in current code, and it is recorded as incumbent rather
  than endorsed. [RFC 6648](https://www.rfc-editor.org/rfc/rfc6648) deprecates the
  `X-` convention for new parameters, and this repository already has a house rule
  for application headers:
  [ADR#0013](./0013-origin-stream-sequence-header.md) keeps them out of the
  server-reserved `Nats-` namespace under a `Trogon-` prefix
  (`Trogon-Origin-Stream-Sequence`). The conforming name is therefore
  **`Trogon-Req-Id`**. Renaming is cheap while the binding is pre-stable and is
  listed under Conformance.

  The `Trogon-` rule governs headers **we name**. A header whose name is fixed by
  an external specification keeps that name, because matching the spec is the
  point: `MCP-Protocol-Version` (MCP), and `AAuth-Requirement`, which
  [ADR#0017](./0017-aauth-agent-authentication.md) carries in the same role and
  format the AAuth draft gives it on HTTP. Reserved `Nats-` remains off limits
  either way.
- `Nats-Msg-Id` is RESERVED for intentional JetStream deduplication, set to a
  stable idempotency key with a deliberately sized `Duplicates` window. It MUST
  NOT be repurposed for correlation.
- Negotiated protocol version, content type, W3C `traceparent`/`tracestate`, and
  large-payload claim-check references travel in headers, never the subject.

#### Negotiated protocol version header

Three different versions exist; only the negotiated protocol version belongs in
a protocol-version header:

| Version | Meaning | Placement |
|---|---|---|
| JSON-RPC `"2.0"` | envelope grammar | **body**, as part of the canonical envelope |
| Negotiated protocol version | payload schema, agreed at `initialize` | **header**, `{PROTOCOL}-Protocol-Version` |
| `v{major}` | subject and routing contract | **subject** |

Header naming is a rule, not a fixed string: `{PROTOCOL}-Protocol-Version`,
derivable from the protocol token that already leads the configured prefix,
upper-cased. The three names are therefore:

- `MCP-Protocol-Version` — fixed by the MCP spec; already emitted and asserted
  by `mcp-nats`
- `ACP-Protocol-Version`
- `A2A-Protocol-Version`

NATS header names are case-sensitive on the wire. Inventing `Acp-` / `A2a-`
would make the family inconsistent with the one member we do not control.

**Emit deferral for ACP and A2A.** Do not emit `ACP-Protocol-Version` or
`A2A-Protocol-Version` yet. Protocol identity is already the trailing token of
the configured prefix, and no ACP or A2A code reads a protocol version at the
transport layer today. Record the rule now; skip the implementation.

**Revisit trigger.** The first time a durable stream must survive a protocol
version bump. ACP's `COMMANDS`, `RESPONSES`, and `CLIENT_OPS` streams are
`Limits` retention with `max_age`, so stored messages outlive their connection.
If a payload shape changes across an ACP version, a replaying consumer has no
way to pick the right schema, and `v{major}` cannot help because it versions
routing, not payload. At that point add the deferred headers per the rule above.

Do **not** put the JSON-RPC `"2.0"` literal in a header.

### Delivery classes — orthogonal to routing shape

Each JSON-RPC method maps to a delivery class, and **any routing shape may use
either class**. A connection-scoped protocol such as MCP MAY be JetStream-backed
for the appropriate methods exactly as a session-scoped protocol is; core-only is
not a property of a protocol.

- **Ephemeral** (core request/reply): handshakes, queries, fast calls. Reply to
  the inbox. No persistence.
- **Durable** (JetStream): long-running commands and replayable
  notifications/events.

Durable responses and notifications MUST be delivered to a subject scoped to the
**bounded entity** (session, connection, or task), with the correlation token
(see Correlation and headers) distinguishing requests *within* that subject. They
MUST NOT mint a per-request subject such as `...response.{req_id}`.

ACP terminal durable results use one subject shape:
`{prefix}.v{major}.session.{session_id}.agent.response`. The historical
`...agent.prompt.response.{req_id}` path collapses into that subject. Partitioning
terminal results by method is unnecessary once demux is on the correlation token.

Mid-flight progress is *not* a durable agent response. `session/update` is a
client-directed notification under the ACP method surface, so it takes the
client-op terminal `{prefix}.v{major}.session.{session_id}.client.session.update`
alongside every other agent-to-client call, and rides the `CLIENT_OPS` stream.
Routing it under `...agent.update` instead would fork one method across two role
segments and scope progress to prompts only, which drops the `session/load`
replay that emits the same notification.

### Streams

- Durable subtrees MUST be wildcard-separable so a stream's subject filter
  captures one class without sweeping ephemeral request/reply traffic.
- A stream's subject set MUST be bounded by entities, not by requests.
- Retention is chosen per class: consume-once uses `WorkQueue` or `Interest`;
  replayable uses `Limits` with `max_age`.
- Use `partition(N, token)` on a fixed entity token when ordered, high-throughput
  processing is required.

### Subscriptions and scaling

- Responders MUST subscribe at the subtree they own (`...{routing-segment}.>`)
  and dispatch the method in process. Per-method and per-request subscriptions
  are prohibited.
- Horizontal scaling uses queue groups (core) or durable consumers (JetStream)
  on the shared subtree. Instance identity MUST NOT appear in the subject; only a
  logical id does. This preserves clean no-responder semantics.

### Authorization and multitenancy

- The primary tenant boundary is the NATS **account**. The company is the
  tenant, aligned with the NATS account, as
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md) records and
  as the A2A auth callout already provisions with tenant-scoped user JWTs.
  Account isolation requires no tenant token in the subject.
- The `{tenant}` component of the dotted prefix exists for deployments where
  several tenants share one account (a shared development cluster, a bridge
  carrying more than one tenant's traffic). There, tenant isolation degrades
  gracefully to a prefix rule because the tenant leads the prefix: allow
  subscribe on `acme.acp.v1.session.*.agent.>` and publish on
  `acme.acp.v1.session.*.client.>`.
- The prefix and `v{major}` are the leftmost structural tokens, so permissions
  within an account are clean prefix rules in either model.
- Within a shared account, reply inboxes use a per-tenant inbox prefix so replies
  cannot cross a tenant boundary; across accounts, the account boundary itself
  isolates inboxes.

### Operational profile

Subjects that are not JSON-RPC protocol bindings — today `a2a.catalog.*`,
`a2a.audit.*`, `a2a.push.dlq.*` — are still NATS subjects and still subject to
this ADR's cardinality, `v{major}`, limits, and left-prefix rules. They use an
**operational profile**:

```text
{prefix}.v{major}.<operational-root>.<bounded-entity...>.<fixed-terminal>
```

- The operational root (`catalog`, `audit`, `push.dlq`, …) is a stable token, not
  a growing vocabulary.
- Terminals are a small fixed set (`register`, `lifecycle`, `mirror`, …), not the
  JSON-RPC method set.
- Entity tokens stay long-lived (agent, caller, task). Per-request identifiers
  remain forbidden.

#### Audit subject defects

Audit subjects MUST be entity-scoped with fixed terminals, for example
`a2a.v1.audit.{agent_id}.{outcome}` (method in the payload or a header), not
method-led growth without an entity token. Two defects motivated the rule:

- `a2a.audit.{outcome}.{method}` grows with the method set and embeds the method
  as a subject token, defeating the fixed-terminal rule.
- An emitter that accepts `agent_id` and then drops it (`let _ = agent_id`)
  leaves audit traffic unfilterable per agent.

Exact terminals are an implementation detail under this profile; the two defects
above are the conformance requirement.

The A2A emitter (`a2a-nats::audit::emitter`) now conforms, publishing
`{prefix}.v1.audit.{agent_id}.{ok|err}` and
`{prefix}.v1.audit.{agent_id}.lifecycle`. The gateway's ingress audit builder
(`a2a-gateway::audit_ingress::ingress_audit_subject`) still emits
`{prefix}.a2a.audit.{outcome}.ingress.{skill}` — duplicated root, no `v{major}`,
skill as a growing terminal, no entity token. It has no production caller today,
so it is corrected when the ingress audit path is wired rather than ahead of it.

### Limits

- At most 16 tokens and 256 bytes per subject (hard ceiling 32 tokens before heap
  escape). Count method arity and any durable suffix against the budget.
- Tokens are lower_snake and case-consistent; subjects are case-sensitive. The
  escape encoding defined under Method-to-terminal mapping is the sole exemption.
- Never use flat subjects; always hierarchical.

### Versioning posture

Two different versions exist; only one belongs in the subject.

- **Payload / message schema** evolves via the protocol's own negotiated version
  (ACP/MCP/A2A, exchanged at `initialize`) carried in
  `{PROTOCOL}-Protocol-Version` when emitted — never the subject. Adding or
  changing a body field MUST NOT change the subject or fragment a subscription.
- **Subject / routing contract** (token layout, method placement, routing-segment
  shape) is versioned by `v{major}` in the subject. It lives in the subject —
  not a header — because NATS interest matching, authorization, JetStream
  filters, and `subject_transform` all act on the subject and cannot see headers.
  Only a subject token lets two versions run side by side, be captured by
  separate streams, be granted per-version access, or be bridged on the wire.

The binding is **not yet at a stable boundary** — nothing depends on it under a
stability guarantee. `v1` is established now as the baseline as part of getting
the layout right; it is not a migration from a prior shipped version. The
bump-and-`subject_transform` discipline — a breaking subject change becomes
`v{n+1}` and MAY bridge old and new on the wire during rollout — applies only
AFTER `v1` is declared stable and has external dependents. Until then, the
alignment below is applied directly.

**Signed subjects do not get the bridge.**
[ADR#0051](./0051-fully-bound-request-signing.md) binds every signature to the
concrete subject the message was published on, and states that subject mapping
which rewrites subjects breaks signatures and is unsupported in front of signed
subjects. So `subject_transform` is a rollout tool for unsigned traffic only. A
version bump on a subtree carrying signed requests is a coordinated
publisher-and-verifier cutover, or it runs both versions side by side until
callers move; it is never bridged by rewriting. This is another reason to get
`v1` right now, while no signed traffic depends on the layout.

## Conformance — Baseline Alignment

While the binding is still pre-stable, the following current behaviors are
corrected **directly** to reach the `v1` baseline, at no migration cost. They are
non-conformant with this standard; they are not the standard:

- **Missing binding-version token** in `mcp-nats`, `acp-nats`, and `a2a-nats`
  subjects. Insert `v{major}` as the first structural token after the configured
  prefix.
- **Request id in ACP response and update subjects**
  (`...agent.response.{req_id}`, `...agent.update.{req_id}`) and the prompt-
  specific `...agent.prompt.response.{req_id}`. Replace with the entity-scoped
  `...agent.response`, correlating on ACP's transport-minted JSON-RPC `id`, and
  collapse the prompt-specific terminal into it.
- **Duplicate ACP progress subject.** `...agent.update` and
  `...client.session.update` both claimed `session/update`, and only the latter
  had a publisher. Remove `...agent.update` and its `NOTIFICATIONS` stream;
  progress rides the client-op subtree.
- **Request id in A2A task event subjects**
  (`...tasks.{task_id}.events.{req_id}`). Replace with
  `...tasks.{task_id}.events` plus `Trogon-Req-Id` (A2A ids are peer-supplied). This
  bounds durable stream subject cardinality to tasks rather than requests.
- **Per-request gateway egress** (`a2a.gateway.egress.{req_id}`). Remove. Live
  streaming uses the reply inbox and entity-scoped JetStream; a core-NATS
  per-request fanout violates the cardinality rule. If a core fanout is later
  required, it MUST be caller-scoped (e.g. `a2a.v1.gateway.{caller_id}.events`)
  with `Trogon-Req-Id`, never `{req_id}` in the subject.
- **ACP substitutes the projected terminal for the real method.** ACP's
  `wire_method()` yields `prompt`, not `session/prompt`, so the method that
  reaches the transport is the subject projection rather than the protocol
  method. Under the canonical body ([ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md))
  the body MUST carry `session/prompt`; only the terminal is projected.
  This one is **not** drift: it is the deliberate outcome of
  [ADR#0022](./0022-canonical-acp-wire-methods-on-nats.md), whose premise [ADR#0056](0056-canonical-jsonrpc-bodies-over-nats.md)
  invalidates. Existing tests assert the old subject-token/`wire_method()`
  unification and must be updated with it, so schedule this with the ACP codec
  migration rather than as a standalone subject fix.
- **`X-Req-Id` violates the header naming rule.** Where the header is still
  required (A2A), rename it to `Trogon-Req-Id` per
  [ADR#0013](./0013-origin-stream-sequence-header.md) and RFC 6648, under one
  constant. The name is currently duplicated in `a2a-nats` and `trogon-nats`; the
  rename collapses them.
- **Undocumented method-to-terminal mappings.** MCP's mapping (case folding plus
  the `custom.{base64url}` escape) and ACP's exist only in code. Each binding
  publishes its mapping per Method-to-terminal mapping.
- **MCP is core-only.** The connection routing shape MUST be able to run
  JetStream-backed delivery classes; do not treat ephemeral-only as inherent.
- **Operational subjects** lack `v{major}` and, for audit, violate entity
  scoping and fixed terminals as noted above.
- **Role / handshake placement divergence** SHOULD be reconciled to the canonical
  grammar where the difference is not semantically required. The
  session-versus-connection-versus-task routing difference is semantic and is
  retained.

Because this is baseline alignment before stabilization, these changes need no
versioned rollout. The bump-and-`subject_transform` discipline (Versioning
Posture) governs only subject-contract changes made AFTER `v1` is declared stable.

## Consequences

- One subject-design standard governs every message-oriented NATS subject this
  repository designs (protobuf micro RPC keeps [ADR#0016](0016-protobuf-rpc-over-nats-micro-binding.md)'s descriptor-derived
  rule, per Scope), so
  MCP, ACP, A2A, and operational roots converge instead of each inventing a
  grammar.
- Subject cardinality stays bounded by entities, keeping the routing cache,
  subscription trie, and JetStream listings healthy at scale.
- Durability becomes a per-method choice available to every protocol, not a fixed
  property of one adapter.
- A mandatory version token gives every binding a migration path from day one.
- The standard exposes concrete, reviewable deviations in current code rather
  than ratifying them.

## References

- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0016: Protobuf RPC over NATS Micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
- [ADR#0017: AAuth Agent Authentication](./0017-aauth-agent-authentication.md)
- [ADR#0022: Canonical ACP Method Vocabulary in the NATS Layer (Rejected)](./0022-canonical-acp-wire-methods-on-nats.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0041: Canonical MCP JSON-RPC Bodies over NATS](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md)
- [ADR#0051: Fully Bound Request Signing](./0051-fully-bound-request-signing.md)
- [ADR#0054: NATS Protocol Binding Documentation](./0054-nats-protocol-binding-documentation.md)
- [ADR#0056: Canonical JSON-RPC Bodies over NATS](./0056-canonical-jsonrpc-bodies-over-nats.md)
- [JSON-RPC 2.0 specification](https://www.jsonrpc.org/specification)
- [NATS subject-based messaging](https://docs.nats.io/nats-concepts/subjects)
- [NATS JetStream model](https://docs.nats.io/nats-concepts/jetstream)
