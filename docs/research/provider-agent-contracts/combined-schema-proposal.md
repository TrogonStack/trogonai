# Combined schema proposal: sessions, agents, vault secrets

Part of the [provider agent contracts research corpus](./index.md).
The four dossiers in [`products/`](./index.md#product-dossiers) read back
against `proto/trogonai/agents/agents/v1alpha1` and
`proto/trogonai/session/sessions/v1alpha1`. Analysis and proposal only: every
change it argues for is gated on an ADR that does not exist yet. Where a
conclusion here differs from an accepted record in the
[ADR index](../../adr/index.md), the ADR is authoritative.

Synthesized from the four per-provider dossiers:
[Anthropic](./products/anthropic-managed-agents.md),
[OpenAI](./products/openai-agents-api.md),
[OpenComputer](./products/opencomputer-serverless-agents.md),
[xAI](./products/xai-platform.md).

This is a **delta against our existing contract, not a greenfield design**. Three
standing facts constrain every section below, and no proposal here may violate
them:

1. Our session aggregate is already richer than every vendor's. We import
   nothing from their session models.
2. Our agent configuration is content-addressed and immutable per revision.
   Nothing here reintroduces mutable agent state.
3. `proto/trogonai/agents/agents/v1alpha1/agent.proto:12` already states the
   boundary: "Grants, credentials, provider routes, live memory, and session
   inputs belong to their own planes and cannot be embedded in this
   declaration." The vault plane is the plane that comment presupposes and
   that does not yet exist.

---

## 1. The headline

Across four providers studied independently, the designs converge on three
things and diverge on one.

**They converge on:**

- **Secret values are write-only.** No read-back path exists for any caller on
  any of the three platforms that have a vault, and OpenAI enforces it in the
  **shape of the schema** rather than by redacting a field (6.7).
- **Credentials attach to the session, not to the agent.** The agent is the
  product-level abstraction; the session is the end-user abstraction.
- **Archive purges the payload and keeps the record; delete keeps nothing.**
- **Material rotates; structural identity does not.** Anthropic locks
  `mcp_server_url`, `secret_name`, `token_endpoint`, `client_id` after creation.
  OpenAI replaces a token "without changing its ID, authentication type, or
  server URL." Two independent designs, one rule. This is the single most
  load-bearing finding in the study and section 6.2 is built on it.
- **The secret never enters the agent process.** Substitution happens at egress.
  Counting the two open-source implementations we already had notes on, this is
  **four independent designs converging on one mechanism** (6.8), which retires
  it as an open design question.

**They diverge on:** how tightly a credential is bound to the destination it may
be used against. This is the axis that actually separates the designs, and it is
the axis our contract should care about most.

The fourth provider, xAI, has none of this, and the reason it has none of this
is the most useful single finding in the study. See section 3.

---

## 2. Cross-provider comparison

### 2.1 Sessions

| | Anthropic | OpenAI | xAI | OpenComputer |
| --- | --- | --- | --- | --- |
| Session is a server resource | yes (`sesn_`) | yes | **no** | yes |
| Requires a pre-created agent | yes | **no**, config may be inline | n/a | no, code is the agent |
| Agent binding | full snapshot at create | saved `agent_id` plus overrides | n/a | pins a deployment absolutely |
| Event log | durable, listable, `sevt_` | durable | **none** | durable, `seq` cursor |
| Event cursor | `processed_at` timestamp, opaque `next_page`, **no monotonic offset** | **none on the event stream at all**; ID-based `after` exists only on the item/turn list endpoints | n/a | monotonic `seq`, starts at 1, `after=<seq>` |
| Stream replay | not replayable, no `Last-Event-ID` | not replayable, "Streams do not replay missed events"; recovery is re-read the item store and reconcile by `item_id` | n/a | poll-based, no stream |
| Durable record | the message list | **the item store, not the event log** | n/a | the event log |
| Chaining across runs | threads, `sth_` | sessions | `previous_response_id`, client-held | turns |
| Lineage (fork, recovery, parent) | not modeled | not modeled | not modeled | not modeled |

### 2.2 Agents

| | Anthropic | OpenAI | xAI | OpenComputer |
| --- | --- | --- | --- | --- |
| Stored agent resource | yes, `agent_` | yes, optional | **no** | no, a source function |
| Versioning | implicit integer, starts at 1 | saved-agent update | skills `version` always `"1"`, nonfunctional | deployment |
| Content-addressed identity | **none** | **none** | n/a | **none** |
| Overrides merge or replace | **replace** | **replace** | n/a | n/a |
| Mutable mid-session | only `tools`, `mcp_servers` | n/a for this study | n/a | nothing, "to run new code, start a new session" |
| Delegation depth | capped at 1 | no documented cap; `max_concurrent_subagents` default 6 | non-addressable sub-agents inside one model | subagents, mechanics undocumented |
| Subagent isolation | own thread, own sandbox per session | **shares the coordinator's environment and filesystem** | none | undocumented |

**Nobody content-addresses an agent configuration.** Anthropic's own study
records this as a gap in their design: "There is no content digest, hash, or
content-addressed identity anywhere in the documented surface, and no way to ask
'which version has this exact config?' other than listing versions and diffing
yourself." We already solved that with `StoredAgentConfiguration` and `Digest`.
This is our largest unforced lead and it needs no change.

### 2.3 Vault secrets

| | Anthropic | OpenAI | xAI | OpenComputer (Agents) |
| --- | --- | --- | --- | --- |
| Vault resource | `vlt_` / `vcrd_` | `vault` / `credential` | **none** | project/agent-scoped secret store |
| Scope | **workspace**, flagged as a hazard | project | n/a | project, per-agent override, per environment |
| Attach point | `vault_ids` at session create | attach to session | inline in every request body | declaration in source |
| Propagates to delegates | yes, "apply to every thread" | yes, subagents "inherit configured MCP tools, their credentials" | n/a | not documented |
| Write-only values | yes, explicit | yes, explicit | n/a | yes, explicit |
| Credential kinds | `mcp_oauth`, `static_bearer`, `environment_variable` | bearer, `mcp_oauth` | n/a | connection-backed, plus runtime vars |
| Destination binding | `mcp_server_url`, or `allowed_hosts` + `injection_location` | MCP server, **service-origin only** | none | `(origin, methods, pathPrefix, agent, project, environment)` |
| Rotation | in place, re-resolved without restart | in place | key rotate with overlap window | in place |
| Archive vs delete | purge payload / keep record, vs hard delete | delete | n/a | remove |

---

## 3. The xAI negative result, and why it matters

`docs.x.ai/openapi.json` has 38 paths and 195 schemas. Verified directly by this
orchestrator, not only by the subagent: there are **zero** occurrences of
`agent`, `session`, `conversation`, `vault`, or `secret` as a JSON key or schema
name.

xAI's agentic product is a client-side coding CLI under `docs.x.ai/build/`
(Sessions, Subagents, Permissions, Sandbox, Worktrees, Hooks, AGENTS.md). Its
MCP credential handling is an inline `authorization` string passed in the request
body on every call.

**The rule this yields:** the vault is not an agent feature. It is a consequence
of hosting someone else's agent against someone else's credentials. A
client-side agent needs no server-side vault because the credentials are already
on the operator's machine. The noun appears in exactly the three products whose
runtime is server-hosted and multi-tenant.

We are in that category. So we need the noun. That is the justification, and it
is stronger than "three competitors have one."

**Risk worth recording:** xAI's `store` defaults to `true` with 30-day retention,
and MCP bearer tokens are passed inline in the request body. Whether stored
responses redact `tools[].authorization` is not documented anywhere. The xAI
study flagged this as unverified and I am carrying it forward as unverified.

---

## 4. Where we are already stronger (record, change nothing)

- **Event-sourced fold with `SessionOrdinal`** ([ADR#0013](../../adr/0013-origin-stream-sequence-header.md)), never a JetStream
  sequence. Anthropic has no monotonic event offset at all: cursoring is by
  `processed_at` timestamp with an opaque page token, and their own docs cannot
  say how ties are broken. OpenAI is weaker still: their event stream carries **no
  cursor of any kind**, "Streams do not replay missed events", and the documented
  recovery procedure is to open a new stream, buffer it, re-read the saved items,
  and reconcile by `item_id`, discarding updates for items already in a final
  state. Their durable record is the **item store**, not the event log, and they
  say plainly that saved items "let you recover completed work, but not every
  intermediate event you missed." OpenComputer has `seq` and is the only vendor
  that matches us here.

  This is worth stating as a design claim, not just a scoreboard. Three of four
  vendors treat the event stream as a **best-effort view over a durable
  projection**. We treat the event log as **the** durable record and fold state
  from it. Ours is the stronger position and it is what makes replay, lineage,
  and `ResourceAccessRecord` auditing possible at all; their designs cannot
  reconstruct what an agent did, only what it finished doing. Delegation makes
  this sharper still: OpenAI's subagent items are retrievable per subagent, but
  they say outright that "the stream does not provide a full conversation
  transcript" and that coordination items can omit message content. So the
  inter-agent record is incomplete by design, which for us would be a defect in
  the audit trail rather than an acceptable simplification.
- **`ResourceAccessRecord` versus `ResourceObservation`.** No vendor separates
  "the agent saw this file's name" from "the agent read this file's content."
  That distinction is a compliance primitive and we are alone in having it.
- **Redaction and erasure with the audit fact surviving the content**
  (`redacted_event_ids`, `erased_artifact_ids`). Anthropic reinvents exactly this
  shape in their vault archive semantics, which is a strong signal we generalized
  it at the right level.
- **Session lineage**: `ForkOrigin`, `RecoveryOrigin`, `ParentLink`,
  `CompactionMarker`, `ExecutionAttempt`. Not one vendor models any of it.
- **Content-addressed configuration**, per 2.2 above.
- **Agent revisions at all.** OpenAI's `Agent` has no `version`, `revision`,
  `etag`, or `digest`; `POST /agents/{agent_id}` mutates in place, and the
  session snapshots only the display `name` ("Later changes to the agent's name
  do not affect this value"), saying nothing about `model`, `instructions`, or
  `tools`. So a team that edits a production agent has no documented way to learn
  which configuration a past session actually ran. The nearest thing to an answer
  is one scoped sentence in their multi-agent guide, "These settings apply at
  session creation. Changes to a stored agent apply to new sessions", which
  implies binding at creation but is stated only about the `multi_agent` settings
  and is never generalized to `model`, `instructions`, or `tools`. Notably they **do** ship
  resolve-and-pin for skills (`version` is "a positive integer or latest", and
  the session echoes "The concrete skill version installed for this session").
  The platform understands the pattern and simply did not apply it to the agent.
  Our immutable, digest-committed revision is the thing they are missing.
- **Human approval is a first-class, invariant-bearing event.** We have the full
  triple: `ToolCallRequested`, then mutually exclusive `ApproveToolCall` /
  `DenyToolCall` recording `ToolCallApproved` / `ToolCallDenied`, with
  `WRITE_PRECONDITION = At` so "a decision taken against a stale head must be
  rejected rather than appended beside its opposite", and a denied call that
  "reserves no operation in the ledger because nothing ran." **OpenAI's Agents
  API has none of this.** Its `required_actions` has exactly two types,
  `function_call` and `environment_connection`, neither of which is an approval
  gate, and `require_approval` / `McpToolApprovalSetting` exist only on their
  Conversations and Responses products. An approval workflow on their Agents API
  has to be built by the application out of a function tool. Same story for
  guardrails, which appear nowhere in their Agents API, and for treating tool
  output as untrusted, for which they publish no provenance field and no
  prompt-injection guidance at all.
- **Credential use is auditable.** No vendor records which credential was
  selected for a given tool call. OpenAI's binding is implicit URL matching, so
  a URL typo "silently produces an unauthenticated call rather than an error."
  Our grant is explicit and admission-resolved, so the selected credential is a
  recorded fact rather than a runtime coincidence.
- **Admission-time pinning** of skills by content digest, tools by exact version,
  delegates by revision number, with moving aliases explicitly invalid.

---

## 5. The gap

```
$ grep -rilE 'secret|credential|vault|oauth|api_key' proto/
```

matches prose comments only. **There is no secrets, vault, or credential
message anywhere in the proto tree.** Not a message, not an enum, not a field.

Meanwhile `agent.proto:12` already promises that credentials live in "their own
plane," and `dependencies.proto` already declares four collections of
dependencies (skills, tools, delegates, memories) with the rule "A declaration
grants no authority."

The gap is not that we lack a vault. It is that we declared a boundary and never
built the thing on the other side of it.

---

## 6. Proposal

### 6.1 Shape: split declaration from grant, reusing the pattern we already have

The single most important design decision here is already made for us by
`dependencies.proto`:

> These declarations are revision-owned. Session admission resolves them under
> live hierarchy and policy, then records exact references and digests. **A
> declaration grants no authority.**

Apply that verbatim to credentials. The plane splits in two:

**(a) A connection declaration, revision-owned, in `AgentDependencies`.**

A fifth collection alongside `skills`, `tools`, `delegates`, `memories`. It names
a `binding_name` and the destination the agent intends to reach. It contains **no
secret, no vault id, and no authority**, exactly like every other declaration.
This is OpenComputer's `defineConnection` idea expressed in our existing idiom:
the destination is part of the immutable, content-addressed, reviewable artifact,
because *which hosts an agent talks to is behavior*, and behavior belongs to the
revision.

**(b) A credential grant, session-scoped, resolved at admission.**

The session binds declared connections to actual credentials in a vault. This is
where `vault_ids` lands, and it is per-session for the reason all three vendors
independently give: the agent is the product, the session is the end user.

This split is what lets us keep both properties at once: the destination is
pinned and reviewable, the secret is late-bound and revocable.

One more argument for keeping the grant separate from the store: **OpenAI's vault
has no authorization model of its own.** The entire `Vault` object is `id`,
`created_at`, `metadata`, `name`, `object`. No ACL, no policy field, no per-agent
restriction, no principal binding. Scoping is assembled from three unrelated
places: the project a vault is created in, the API key's `api.vaults.read` /
`api.vaults.write` permissions, and the per-session `vault_ids` opt-in. The store
is a bag; all the authority lives outside it. That is a workable decomposition and
it is close to what 6.1 proposes, but it is worth naming the consequence they
accepted: because nothing on the vault says who may use it, the answer to "which
agents can reach this credential" cannot be read off any single object. A
declaration plus grant split gives us the same decomposition while keeping that
question answerable from the artifact.

### 6.2 The pin rule and its single exception

Our contract pins everything at admission. Credentials must be the **one
exception**, and the proposal should say so explicitly rather than leaving it
implicit.

Evidence: Anthropic re-resolves credentials periodically "both during a session
and during the vault lifecycle... so that credential rotation, archival, or
deletion propagates to running sessions **without a restart**." OpenComputer's
sandbox rotation "needs no restart." Our own `decision-record.md` already
independently concluded: "The one sanctioned live mutation: credential rotation."

A pinned credential cannot be revoked. If revocation does not bite a running
session, the vault is decorative. So:

> A session pins the **credential reference and its structural identity**. It
> never pins the **secret material**, which is resolved per use and may change or
> vanish underneath a running session.

Anthropic's design shows how to make that safe: structural fields
(`mcp_server_url`, `secret_name`, `token_endpoint`, `client_id`) are **locked
after creation**, so a credential's identity is stable across rotations even
though its payload is not. Adopt that. It is the piece that makes late binding
compatible with an immutable audit trail.

OpenAI reached the same rule independently, which is the strongest evidence in
this whole study. Their rotation endpoint replaces a token "**without changing
its ID, authentication type, or server URL**." Two vendors, with different
architectures and no shared spec, both concluded that **material rotates and
structural identity does not**. That convergence is what promotes the rule above
from a reasonable design choice to the obvious one.

Two consequences of late binding that both vendors document, and that we should
adopt rather than rediscover:

1. **Deletion is not revocation.** OpenAI is blunt: "Deleting stored credentials
   does not revoke the original tokens with their providers **or stop a running
   session**." Removing our grant must therefore be specified as removing *our*
   ability to present the secret, never as a promise about the upstream provider
   or about work already in flight. If we want a running session stopped, that is
   a separate, explicit act.
2. **Expiry is not deletion.** "Token expiry does not delete the credential or
   its vault." An unusable credential and an absent credential are different
   states, and admission has to be able to tell them apart to produce a useful
   error.

### 6.3 Destination binding: adopt the tight end of the spectrum

The four providers sit on a spectrum of how tightly a credential is bound to
where it may be used:

```
loose                                                              tight
  |                                                                   |
 xAI            OpenAI              Anthropic              OpenComputer
inline in    vault -> MCP        cred -> mcp_server_url    cred -> (origin,
every         server, service-    or (secret_name +         methods, pathPrefix,
request       origin only         allowed_hosts +           agent, project,
body                              injection_location)       environment)
```

OpenAI's position on that spectrum is set by a rule worth stating on its own,
because it is really about **residency**, not tightness. Their vault "stores
credentials for MCP connections **from OpenAI**", and they send you elsewhere for
the other case: "For connections **from your environment**, use the other MCP
authentication options." The tool carries `connection_origin: "service"` to say
which side opens the socket. So OpenAI does not have one credential model with a
loose binding; it has two disjoint models split by who makes the call, and the
vault only governs one of them. That is exactly the open question logged as D7,
and it now has a named precedent: **origin is a property of the connection
declaration, and it selects which credential mechanism is even applicable.**

We should sit at the OpenComputer end, because that end is the one our existing
design can actually express. Their six-way check ("the credential is attached
only after the destination, method, path, agent, project, and environment have
been validated") is a natural fit for a declaration that already lives in an
immutable, digest-committed artifact.

Two OpenComputer rules worth adopting outright:

- **Undeclared destinations have no injection path at all.** Injection happens
  inside the declared connection, so an arbitrary outbound call simply carries no
  credential. Fail-closed by construction beats fail-closed by policy check.
- **Reject hard-coded sensitive headers at declaration time.** They refuse
  `Authorization`, `Cookie`, `X-API-Key` in a declaration and discard request
  headers that could override managed credentials. This is a static gate on an
  artifact we already validate at admission.

One place we should deliberately **not** follow OpenAI: their resolution is
implicit and ambiguity is resolved late. "The Agents API selects a credential
that matches the server URL. If several attached credentials match, set the MCP
tool's `credential_id` to select one." Our `dependencies.proto` already took the
opposite line for every other collection: "Multiple matching resources fail
admission, even for optional declarations." Keep that. A credential is the last
thing that should be chosen for us by a match heuristic, and an ambiguous grant
is a bug that should surface at admission rather than at first egress.

### 6.4 Egress substitution is the mechanism we have no analogue for

Anthropic's `environment_variable` credential type is the single most interesting
mechanism in the whole study:

> stored in the sandbox as an opaque placeholder. When the agent initiates an
> outbound request, the opaque placeholder is substituted with the real secret at
> egress. **The agent never sees the secret value.**

Scoped on two independent axes: `networking.allowed_hosts` (max 16) says *which
hosts* the secret is substituted for, `injection_location` (`{header, body}`)
says *which part of the request* it lands in. Their docs are explicit that these
are separate axes on purpose, and that `allowed_hosts` "controls which requests
use the secret, not which requests are allowed" - the environment network policy
is a second, independent gate that must also permit the host.

OpenComputer does the same thing with a sealed placeholder (`osb_sealed_...`) and
states the security property plainly: "Bypass fails closed."

The property this buys is the one that matters: **a compromised agent process
cannot exfiltrate the secret, only use it against hosts already allowlisted.**

We have no representation for this. It should be in the plane from the start,
not retrofitted, because it changes the shape of the credential message (a
credential needs a substitution policy, not just a value).

### 6.5 Archive versus delete: reuse, do not invent

Anthropic: archive "purges secrets; records are retained for auditing"; delete is
"hard delete, the record is not retained."

That is precisely our existing artifact erasure model, where
`erased_artifact_ids` keeps the audit fact that something existed while the
content is destroyed. We do not need a new concept. We need to apply the one we
own to a new plane, which keeps the proposed surface materially smaller than it
first appears.

OpenAI converges on the same two states and gives us a cautionary tale about
specifying them loosely. Their `VaultStatus` is `"active"` or `"archived"`,
described as applying to "a vault or credential", and `GET /vaults` filters on it
(`status=active`, or `status[]=active&status[]=archived`, "Both statuses are
included by default"). Yet **neither the vault object nor the credential object
documents a `status` field, and no archive operation is documented anywhere.**
A client can filter by a state it cannot read and cannot cause. Whatever we do,
the state, the transition that produces it, and the field that exposes it should
land in the contract together.

### 6.6 What we deliberately do NOT copy

- **Anthropic's workspace-scoped vaults.** Their own docs carry it as a Warning:
  "any API key with workspace access can reference them when creating a session.
  To revoke access, delete the vault or credential." Their study records that
  there is no per-agent or per-session ACL and that destroying the credential is
  the only revocation primitive. We have [ADR#0050](../../adr/0050-signed-first-caller-authentication.md) (signed first-caller
  authentication) and [ADR#0051](../../adr/0051-fully-bound-request-signing.md) (fully bound request signing), which let a vault
  reference be bound to a specific attested caller. We should use them. This is a
  deliberate divergence and the proposal should record it as such.
- **OpenAI's restriction of vaults to service-origin MCP only.** Their docs send
  you elsewhere for "connections from your environment." That is the same
  service-origin versus environment-origin axis already filed as D7 in
  `docs/research/agent-platform/contract-impact-agents-api.md`, now shown to have
  a secrets consequence and not only a reachability one. D7 should be reopened
  with this evidence.
- **Anthropic's unversioned environments.** Their docs admit the hole: "keep your
  own record of the changes so you can tell which configuration each session
  used." If we add an environment noun (D1), it must be versioned or digested.
- **OpenComputer's agent runtime variables.** They are an explicit escape hatch
  that hands the agent plaintext, and their docs say plainly that they "do not
  provide the destination isolation of managed secrets." We already forbid this
  shape via [ADR#0048](../../adr/0048-one-time-plaintext-exposure.md). Do not add it.

### 6.7 Express write-only in the schema, not in a redaction rule

OpenAI enforces write-only with a technique worth copying exactly: **the create
type and the read type are different types**, and the read type simply has no
field for the secret. There is no masked placeholder and no `REDACTED` sentinel.
The same trick is applied to hosted environment `setup_commands`: "Ordered,
confidential setup commands. **Command bodies are never returned.**"

For us this is a concrete proto directive. Do not define one credential message
with a `value` field plus a rule saying the server blanks it on read. Define the
secret-bearing input as its own message that only ever appears in a request, and
give the stored/returned record no such field at all. A field that cannot be
populated cannot be leaked by a new code path, a debug dump, or a proto reflection
tool, and it makes the guarantee checkable from the schema alone rather than from
handler discipline. This is the same argument as `Digest` carrying
`LEGACY_REQUIRED` presence: put the invariant where it cannot be skipped.

One consequence to accept knowingly: a client then cannot tell whether a secret
was ever set, beyond the credential existing. That is the correct trade.

### 6.8 Prior art we already hold, and what it changes

Two items already in our own knowledge base bear directly on this proposal. Both
predate the vendor study and both independently arrived at mechanisms above.

**agentgateway `BackendAuthCredential`** (agentgateway/agentgateway#2316). The PR
first shipped `backendAuth.headers: [{name, prefix, secretRef}]`. A maintainer
pushed back and asked for `credentials: [{location, secretRef}]`, reusing an
existing `AuthorizationLocation` type, with the stated reason that **query
params and cookies should not need parallel API fields later**. The shipped
proto is:

```proto
message BackendAuthCredential {
  AuthorizationLocation location = 1;
  string value = 2;
}
```

This should change our design. Anthropic's `injection_location` is `{header,
body}`, an enum, and copying it would bake in exactly the shape that review
rejected. **Model the injection point as a value object, not an enum.** A
location is "header `X-Api-Key` with prefix `Bearer `", or a query parameter, or
a cookie; those are not three values of one scalar. This is the Primitive
Obsession rule applied to the credential plane, and we have an external review
thread showing what it costs to get it wrong and then have to widen it.

**OneCLI** (github.com/onecli/onecli, Apache-2.0) is a credential vault plus
egress proxy for agents, and is therefore a **third independent implementation of
the substitution mechanism in 6.4**, alongside Anthropic and OpenComputer. It is
the only one whose source we can read. Its injection rule is host pattern plus
path pattern plus an ordered list of actions:

```json
{ "path_pattern": "/v1/*",
  "injections": [
    { "action": "set_header", "name": "x-api-key", "value": "<real key>" },
    { "action": "remove_header", "name": "authorization" } ] }
```

The `remove_header` action is the part worth stealing. No vendor documents it,
yet it closes a real hole: a client-supplied `Authorization` header that would
otherwise ride along next to the injected one. OpenComputer gestures at the same
concern from the declaration side by discarding "request headers that could
override managed credentials." So substitution is not one operation, it is
**set plus strip**, and a credential's substitution policy needs to express both.

Counting OneCLI, the tally for egress substitution is four independent designs
(Anthropic, OpenComputer, OneCLI, agentgateway) converging on the same shape.
That is no longer a vendor feature to evaluate. It is the default design, and
the burden of proof sits on any alternative.

### 6.9 Custody tiers are already decided

[ADR#0033](../../adr/0033-two-tier-key-custody-product-model.md) fixes exactly two customer-facing tiers, managed key and customer
managed key, and states that tier is registration ownership rather than provider
choice. Nothing in this study disturbs that, and no vendor offers a comparable
customer-controlled-KEK story for agent credentials. The vault plane consumes
[ADR#0033](../../adr/0033-two-tier-key-custody-product-model.md); it does not restate or extend it.

### 6.10 [ADR#0048](../../adr/0048-one-time-plaintext-exposure.md) is externally validated

Two independent implementations state our accepted rule almost verbatim:

- OpenAI: "Retrieving a vault or credential does not return its secret values."
- Anthropic: the supplied values "are treated as sensitive, write-only fields and
  never returned in API responses."

[ADR#0048](../../adr/0048-one-time-plaintext-exposure.md)'s "Plaintext appears exactly once, in the direct response to the request
that created it. Everything else is metadata" is the converged industry contract.
Cite it as validation and do not relitigate it.

---

## 7. Open questions, each wanting its own ADR

Carried forward from `contract-impact-agents-api.md`, now with new evidence:

- **D1, no environment noun.** All three server-hosted providers have one and it
  is load-bearing for secrets: Anthropic's environment network policy is an
  independent gate on credential substitution. Unchanged in priority, but the
  secrets evidence strengthens it.
- **D3, steering unrepresented.** OpenAI supplies the cleanest evidence that this
  is a genuine fork in the road, because they answered it **both ways in one
  company**. In the Agents API, steering is not a verb: "A message sent during an
  active turn steers that turn", with no endpoint, no event type, and no priority
  flag, only a refusal code `active_turn_not_steerable`. Their Responses API
  instead ships a full protocol: `response.steer`, `response.steer.pending`,
  `required_input`, `too_many_pending_steers`. Their docs never reconcile the two.
  We should pick deliberately rather than drift, and the existence of
  `too_many_pending_steers` is a hint that the explicit version acquires queueing
  semantics we would then owe an answer for. Worth noting how far OpenAI pushed
  the implicit reading: **one** endpoint, `POST /agents/sessions/{id}/events`,
  carries all three input types, `agent.session.input.message` ("Adds one or more
  user messages and starts a turn"), `agent.session.input.cancel`, and
  `agent.session.input.tool_result`. Submitting work, steering it, and cancelling
  it are one surface distinguished only by payload type, and the disposition
  ("starts a turn" versus "steers that turn") is decided by session state rather
  than by the caller.
- **D4, subagent versus child session.** Now the best evidenced of the deltas.
  Anthropic caps delegation depth at 1. OpenAI's subagents are **in-session**:
  "The coordinator and subagents **share its filesystem**. Creating a subagent
  does not create another environment", `max_concurrent_subagents` defaults to 6
  excluding the coordinator, and the whole lifecycle is expressed as items
  (`create_subagent_call`, `send_subagent_input_call`, `wait_for_subagents_call`,
  `interrupt_subagent_call`, `resume_subagent_call`, `close_subagent_call`) with
  events `agent.session.subagent.created/active/closed` and a turn field
  `subagent_id` that "is `null` for the main agent". Crucially they also inherit
  credentials (see V6). OpenComputer does not document subagent mechanics at all.
  Our only primitive is a child session, which shares **no** workspace, so we are
  not choosing between two spellings of one concept: we lack the cheap in-session
  fan-out that two vendors treat as the common case.

  A second fork inside D4, and the one that actually touches our schema: **who
  owns the delegation tools.** OpenAI's answer is the harness. "The harness
  supplies tools to create, message, wait for, and interrupt subagents. **You do
  not declare these tools yourself.**" Delegation is switched on by a single
  boolean, `multi_agent.enabled`. Our answer is the opposite: `DelegateDeclaration`
  makes each delegate an explicitly declared, revision-owned dependency pinned by
  revision number. Theirs is one switch and an implicit toolset; ours is an
  enumerated allowlist. Ours is the better fit for an auditable artifact, but we
  should adopt it knowingly, because it means we can never offer their
  "just turn it on" ergonomics, and the cost lands on whoever authors the agent.
- **D5, `OPERATION_KIND_ENVIRONMENT_PROVISION`.**
- **D7, MCP connection origin and residency.** Reopen with the secrets evidence
  in 6.3, which now has a named vendor precedent rather than only our own
  reasoning.

New from this study:

- **V1. Does a credential grant belong in the event log, and in what form?**
  A grant is session state and must be auditable, but no plaintext may ever enter
  an event payload, a projection, or a digest input. The likely answer is a
  reference plus structural identity only, mirroring `ResourceObservation`'s
  claim-check idiom. Note that no vendor records which credential was actually
  used for a given call, so there is no prior art to copy and the decision is
  entirely ours.
- **V2. What happens to a running session when a credential is revoked
  mid-flight?** The two vendors that document it disagree outright. OpenAI: a
  deleted credential does not "stop a running session", and cancellation is the
  application's job. Anthropic: deletion propagates to running sessions "without
  a restart". This is the practical meaning of the pin rule in 6.2, so it cannot
  be left implicit. Recommendation: Anthropic's behavior with OpenAI's honesty.
  Propagate to running sessions, and state plainly that we make **no** claim
  about the upstream provider's own revocation, because we cannot keep one.
- **V3. Conflict when two grants match the same destination.** Anthropic
  explicitly does not document this. OpenAI matches credentials to servers by URL
  and resolves collisions with an optional `credential_id` on the tool, which
  also means a URL typo "silently produces an unauthenticated call rather than an
  error." We should not inherit that failure mode. "Multiple matches fail
  admission" is already our house answer for `ToolSelector` and
  `DelegateSelector`, and a credential is the worst possible place to start
  guessing.
- **V4. Egress substitution requires a proxy in the data path.** That is an
  architecture commitment, not just a schema one. OpenComputer publishes the
  threat-model regression from moving their proxy inside the guest: "root in the
  guest still could not reach the proxy. Here, it can." Whatever we choose, the
  blast radius should be written down the way they wrote theirs down.
- **V5. Do credential grants cross the delegation boundary?** Both vendors say
  yes and both say it in passing: Anthropic's vaults "attach per-session and
  apply to every thread"; OpenAI's subagents "inherit configured MCP tools,
  **their credentials** and allowed tools." Inheritance is nearly free for them
  because their subagents live inside the parent session. Our only delegation
  primitive is a **child session**, which makes the same question load-bearing
  and unanswered: does creating a child session inherit the parent's credential
  grants, narrow them, or start empty? This interacts directly with D4, and
  "start empty" is the only answer that is safe by default.
- **V6. Is `StoredAgentConfiguration` still ownerless?** Open review item from
  earlier work, unrelated to this study but still unanswered: it is referenced by
  nothing outside its own definition, so the artifact store its retention rule
  describes has no named owner, writer, or reader.

---

## 8. Sequencing

Next free ADR number is **0063**.

1. **The vault plane itself** (6.1 through 6.5), one ADR. It is the only item
   here that closes a stated-but-unbuilt boundary, and everything else in this
   document is either a divergence note or an existing open item.
2. **V2 and V3** fold into that ADR as decided semantics rather than separate
   records; both are small and both are questions the vendors left unanswered.
3. **D7 reopened** with the origin-plus-secrets evidence.
4. **V4** as an architecture record once the plane's shape is settled.
5. **D1 and D5 together**, unchanged from the existing sequencing.

Nothing in this document proposes changing the session aggregate or the agent
configuration. The delta is one new plane and one declaration collection.
