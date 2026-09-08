# The Policy Decision Point Behind ACP's session/request_permission for trogonai

Synthesis over agent-identity research, the ACP bridge-mechanics findings,
and trogonai's own ADRs and aauth crates (read at `tmp/trogonai`, main,
`058b8bee`). Produced 2026-07-30 by a source-grounded research agent;
single-pass but every claim carries a file path or URL.

## Key claims

1. `rsworkspace/crates/acp/acp-nats/src/client/request_permission.rs`
   (`forward_to_client`) validates only that `request.session_id` matches
   the NATS subject's session id, then calls
   `client.request_permission(request)` with no identity join, no policy
   evaluation, and no audit trail beyond a `warn!` on error. Confirmed
   blind passthrough.
2. ADR 0032 verbatim: "the current ACP provider operations are not a safe
   place to define this boundary. Their authorization headers are arbitrary
   data, and the global provider selection method does not identify the
   platform Session whose grant would authorize the request." Written about
   model-provider credentials, but it generalizes: any field arriving
   inside an ACP message is untrusted payload, not an authorization
   boundary.
3. ADR 0037 draws the load-bearing distinction: verification ("is this
   signature valid," decentralized, offline) vs authority ("may this key
   act now," governed: admission, revocation, tenancy). A permission
   decision point lives entirely on the authority plane and consumes an
   already-verified identity; it never does its own crypto.
4. trogonai has a working precedent for the hook shape: ADR 0026's
   `CommandAuthorizer<C: Decider>` trait, which runs after state load but
   before `decide`, defaults to allow-all, and fails closed
   (`Unauthorized`) when a principal is missing or invalid.
5. The `aauth` crate family already contains the building blocks, unused
   for this purpose: the `act` delegation-chain verifier
   (`trogon-aauth-verify/src/delegation.rs`, structurally RFC 8693's `act`
   claim / Uber's actor chain), the `AAuth-Capabilities` header
   (`trogon-aauth-sdk/src/capabilities.rs`), the `OrganizationPolicy` trait
   returning `Issue/RequireClaims/Deny` (`trogon-aauth-as/src/policy.rs`),
   and `ApprovedMission` (`trogon-aauth-person/src/mission.rs`: approver,
   agent, approved_tools, capabilities), a pre-existing internal analogue
   of an approval ladder that is unaware of ACP.
6. [Bridge mechanics](./bridge-mechanics.md) already
   prescribes the target ladder: channel-native interaction where the
   platform can render it, policy engine where it cannot, deny-by-default
   on timeout (Hermes maps allow_once/allow_session/allow_always/deny onto
   its own tiers and denies on timeout).
7. Uber's production architecture (STS, per-hop short-lived
   audience-scoped JWTs, actor-chain attestation carried forward) runs at
   P99 under 40ms: evidence a per-call policy/token hop fits a per-tool-call
   latency budget.
8. agentgateway's OBO/token-exchange pattern shows the credential-isolation
   shape ADR 0032 requires: only the broker holds secrets; the agent
   process and the tool it calls never do. "No secrets in ACP" is
   achievable without weakening delegation.

## 1. Decision inputs

A `session/request_permission` request carries `session_id`, a tool
name/kind, and command text (treated as sensitive: Discord approvals
default to DM delivery "because approval prompts include the command
text"). That is all ACP hands the client.

What trogonai must join to it, none of which ACP carries:

- **Agent identity**: the self-certifying public-key identity anchored at
  genesis (ADR 0036), bound by proof-of-possession on the AAuth plane
  (ADR 0017) before `decide` runs (ADR 0026), plus current authority
  status: not-revoked, tenancy-scoped (ADR 0037).
- **Delegation chain**: who this agent acts for, via the `act`-chain shape
  already implemented in `trogon-aauth-verify/src/delegation.rs`
  (recursively nested upstream actors, capped at `MAX_CHAIN_DEPTH`).
- **User/channel context**: the session key already carries
  platform:chat_type:chat_id:thread_id:user_id (bridge-mechanics), the
  human-in-the-loop side of the decision.
- **Workspace/tool risk class**: which stream/tenant/workspace the tool
  call touches, plus a per-tool risk classification (matches Uber's
  mandated risk classes for high-risk systems).

ADR 0026's `CommandPrincipal` (kind: agent/person/service, stable id,
opaque claims/scope) is close to the needed tuple; it lacks a
channel/session dimension today.

## 2. Policy engine options

The Cedar-via-AuthZEN mental model: AuthZEN standardizes how an
authorization question is asked (PEP to PDP, subject/action/resource/context);
Cedar (or OPA, or homegrown rules) is a separable choice for computing the
answer. Composition, not competition.

For a Rust NATS platform with per-tool-call latency budgets: adopt the
PARC-shaped decision model (Principal, Action, Resource, Context) as the
input schema, but do NOT adopt AuthZEN's wire protocol yet (no external
PEP/PDP split exists). The in-repo precedent is
`trogon-aauth-as/src/policy.rs`'s `OrganizationPolicy`
(`decide(ctx, trust) -> Issue|RequireClaims|Deny`), documented as
synchronous and pure, with external call-outs happening before context
construction; that keeps the core decision path free of I/O and protects
latency. A homegrown Rust trait mirroring
`CommandAuthorizer`/`OrganizationPolicy` beats a Cedar/OPA integration
today; Cedar/AuthZEN becomes relevant if trogonai later exposes policy
evaluation to external resource servers (the Okta/Keycloak ID-JAG shape).

## 3. Delegation chains

The chain is user -> channel adapter -> agent -> tool. trogonai has native
primitives for two of the three hops:

- **Channel adapter -> agent**: not yet formalized as a delegation
  credential; implicit in the session key today. This is the gap relative
  to Uber's model, where every hop mints a fresh audience-scoped
  short-lived token and folds the growing actor chain into `act`.
- **Agent -> tool**: exactly `trogon-aauth-verify`'s `act` chain, whose
  `flatten_act_chain` output is documented as feeding "audit logging or
  policy evaluation"; it is simply not wired to a policy point yet.
- **Scoped credential minting**: per ADR 0032's supervisor pattern and the
  agentgateway OBO/RFC 8693 examples, mint at the narrowest point (broker
  or supervisor), never hand credentials to the agent process; the checker
  (section 5) is a different component from the minter.

The ID-JAG `act` claim (attributing an exchange to the specific agent
client acting for the user, distinct from the redeeming client_id) and
Uber's STS actor-chain tokens both show "delegate at every hop,
chain-attest the lineage" as the converged industry pattern.

## 4. Approval ladder integration

Treat the bridge-mechanics ladder as settled direction: platform-native UI
(Telegram buttons, Discord components, WhatsApp reactions) where the
channel renders interactive prompts; policy engine (permissionMode x
nonInteractivePermissions, mapping allow_once/allow_session/allow_always/
deny) where it cannot; deny-by-default on timeout in both, never
fail-open. `ApprovedMission` (approver, agent, approved_tools,
capabilities) already looks like an allow_session/allow_always grant
record and is a stronger foundation for the remembered tiers than a new
ACP-side cache. Audit per ADR 0032 section 10: log principal, delegation
chain, tool/resource, decision, correlation id; never the credential or
grant material.

## 5. Recommendation

Add a `PermissionAuthorizer` trait in the acp-host/acp-nats client layer,
modeled on ADR 0026's `CommandAuthorizer`, invoked between the current
session-id check and `client.request_permission`:

- Accepts a typed context: verified agent identity (ADR 0036/0037), `act`
  delegation chain (computable via `trogon-aauth-verify`), session/channel
  key, tool risk class. Never raw ACP fields.
- Returns `Issue { scope }` / `RequireInteractive` (fall through to the
  channel-native prompt) / `Deny { reason }`, mirroring
  `trogon-aauth-as::policy::Decision`.
- Defaults to `RequireInteractive` (today's passthrough behavior), like
  ADR 0026's `AllowAll` default, so call sites keep compiling.
- Consults/extends `ApprovedMission`-shaped state for
  allow_session/allow_always tiers instead of new storage.
- Logs every decision (principal, chain, tool, resource, outcome,
  correlation id) without command text or credential material.
- Fails closed on timeout, missing/unverifiable principal, and
  policy-engine unavailability. Never falls back to allow.

Open questions worth their own ADRs:

- Should the channel-adapter -> agent hop mint its own scoped delegation
  record (a trogonai-native `act` entry), or does session-key context
  suffice until adapters call other agents directly?
- Extend `ApprovedMission` to cover ACP tool grants, or introduce a
  sibling type to avoid overloading its AS/PS-handshake semantics?
- Policy call-out inline in acp-host (latency, per OrganizationPolicy's
  synchronous-by-design note) or as a NATS-addressable service (reuse
  across A2A and future external resource servers)?
- Eventually expose an AuthZEN-shaped endpoint for external PDPs, or is a
  homegrown trait sufficient indefinitely?

## Sources

Prior research: "Do AI Agents Need Their Own Identity?", "Agentgateway adds
token exchange, jwt-assertion, and Entra OBO", "Okta SAML and Keycloak for
ID-JAG Cross App Access", "On-Behalf-Of, Explained", "Cedar-Powered
Fine-Grained Authorization in Keycloak via AuthZEN", "Solving the
Identity Crisis for AI Agents" (Uber). [Bridge
mechanics](./bridge-mechanics.md). trogonai (main,
`058b8bee`): ADRs 0017, 0026, 0032, 0036, 0037, 0038, 0039;
`rsworkspace/crates/acp/acp-nats/src/client/request_permission.rs`,
`client_handler.rs`; `rsworkspace/crates/aauth/trogon-aauth-verify/src/
delegation.rs`, `trogon-aauth-sdk/src/capabilities.rs`,
`trogon-aauth-as/src/policy.rs`, `trogon-aauth-person/src/mission.rs`.
