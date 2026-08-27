# Vercel Connect: Credential Sprawl Research and TrogonAI Adoption Notes

## Status and scope

This is a research and adoption note, not an ADR. Repository ADRs remain the
decision authority wherever this note proposes a change.

- Primary article: [The end of credential sprawl for agents](https://vercel.com/blog/the-end-of-credential-sprawl-for-agents)
- Published: August 25, 2026
- Authors: Hedi Zandi, Ben Sabic, and Dima Voytenko
- Research snapshot: August 25, 2026
- Repository scope: the current `west-monroe` checkout, including the working
  copy of `CREDENTIAL_PLATFORM_SPEC.md`

## Executive conclusion

The useful idea in Vercel Connect is not a better vault. It is a credential
access broker above a vault:

1. durable provider grants and root credentials stay in a trusted control
   plane;
2. a workload proves its identity when it needs access;
3. the broker authorizes the exact subject, provider installation, environment,
   and requested authority;
4. the broker exchanges or refreshes durable material into a short-lived
   provider token where the provider supports that; and
5. the runtime uses the temporary token without receiving the durable grant.

That is a material improvement over copying long-lived tokens into application
configuration. It does not end credential storage. OAuth refresh tokens, OAuth
client secrets, webhook secrets, and static API keys still need custody. It
also does not guarantee that every returned credential is short-lived, scoped,
or revocable because those properties depend on the provider.

TrogonAI should keep OpenBao and the accepted secret-custody decisions. The
new lesson is to add a typed provider-access broker instead of making raw
`SecretStoreGet` the normal outbound access path. For untrusted agent execution,
TrogonAI should be stricter than Vercel Connect: the native agent should receive
neither the durable credential nor the temporary provider token. A trusted
supervisor, tool executor, or egress proxy should apply the credential and make
the provider call.

The repository already contains most of this stronger model for model-provider
access in draft ADR#0032. The right move is to reuse that connection, binding,
short-lived grant, and proxy model for tools, channels, remote MCP servers, and
other provider calls instead of creating a second credential architecture.

## The problem the post identifies

A vault protects a credential while it is stored and controls who may retrieve
it. Once a long-lived provider token is copied into an environment, sandbox,
agent process, or configuration file, its provider-defined lifetime and
authority determine the blast radius. Rotation scripts and synchronized copies
reduce operational friction but do not change that property.

The precise conclusion is:

> A vault is necessary for durable custody, but it is not sufficient as the
> runtime authorization plane for provider access.

This needs one qualification. OpenBao can issue leased dynamic credentials and
can perform cryptographic operations without releasing key material. The blog's
critique applies most directly to static third-party credentials that are read
from a vault and then used elsewhere.

## What Vercel Connect is

Vercel Connect is a team-level connection, consent, token-exchange, and webhook
broker. The GA announcement says it supports more than 100 connector presets,
with managed connectors for a smaller set of providers and custom OAuth, API
key, and OAuth-capable MCP connections for the rest.

The number of presets must not be read as more than 100 fully managed OAuth
applications. Many entries provide known metadata, setup guidance, or a
customer-managed connection method.

### Core object model

| Object | Meaning | Security role |
| --- | --- | --- |
| Connector | Team-owned record for one external service and authentication method | Holds provider setup and determines supported capabilities |
| Installation | One provider tenant grant, such as a Slack workspace or GitHub organization | Selects the external tenant the token may reach |
| Project link | Association between a connector, a Vercel project, and allowed environments | Authorizes which deployment identities may request tokens |
| Authorization | App or user consent recorded against a connector | Establishes provider-side delegated authority |
| Subject | `app`, named `user`, or federated `jwt-bearer` | Chooses whose provider identity the token represents |
| Token request | Connector plus subject, installation, scopes, resources, audience, and authorization details | Narrows one runtime request |
| Provider credential | Credential returned to server-side application code | Authorizes the subsequent provider API call |
| Trigger | Verified provider webhook forwarded to a configured destination | Covers the inbound half of the integration |

### Connector ownership models

- A Vercel Managed Connector uses an OAuth application registered by Vercel.
  The customer still authorizes or installs it in the provider tenant.
- A Customer Managed Connector uses an OAuth client or API key supplied by the
  customer. Vercel stores the credential and brokers access.
- Assisted Setup performs supported registration steps once. It does not turn a
  customer-managed connector into a Vercel-managed connector.

### Runtime credential flow

```text
deployed application
  -> authenticates to Connect with Vercel OIDC
  -> requests connector + subject + installation + provider authority
  -> Connect checks team, project, environment, connector, and authorization
  -> Connect exchanges, refreshes, or retrieves provider credential material
  -> Connect returns the provider credential for that connection method
  -> application calls the provider directly
```

The normal caller credential is a Vercel-issued workload OIDC token. For local
development, `vercel env pull` writes an approximately 12-hour token into
`.env.local`. Non-Vercel runtimes use a Vercel access token, which is another
standing credential that must be protected.

The workload identity proves the Vercel team, project, and environment. It does
not by itself prove the application's end user, the agent's task, or the
business justification for a provider action.

### Token request and lifecycle

A request may contain:

- an app, user, or federated subject;
- a provider installation;
- provider scopes;
- provider resource indicators;
- an audience;
- rich authorization details; and
- a validity buffer or force-refresh instruction.

The SDK uses an in-process LRU cache with 100 entries. The cache key includes
the connector and request parameters. A token is refreshed when it enters the
default 30-second validity buffer.

User-subject tokens require a consent flow. Vercel stores the resulting refresh
token on its infrastructure and performs later refreshes. Provider access
tokens are returned to the application and therefore remain bearer secrets for
their lifetime.

Revocation has two distinct outcomes:

1. Connect can immediately refuse future issuance and remove its stored token
   state.
2. An already-issued provider token is invalidated immediately only when the
   provider exposes and honors a revocation operation. Otherwise it remains
   usable until its provider-defined expiry.

### Project and environment binding

A project link authorizes selected deployment environments to request from a
connector. It controls issuance, not the authority embedded in a token after it
has been issued.

Environments linked to the same connector may still reach the same provider
installation. Vercel recommends separate connectors when production and test
need provider-level isolation. This distinction maps directly to TrogonAI's
accepted decision that environment is an attribute rather than a hierarchy
level.

### Inbound triggers

Vercel Connect also receives provider webhooks, verifies the provider
signature, and forwards the event to configured project destinations. The blog
describes the forwarded event as re-attested with OIDC. The trigger
documentation separately requires the receiver to verify a signature using a
published per-connector signing key. The exact receiver contract should be
confirmed before treating those descriptions as interchangeable.

The documented trigger behavior includes:

- explicit destinations only;
- at most three destinations per connector;
- up to three attempts for selected server errors;
- separate lifecycle for trigger destinations and project links; and
- receiver-side verification before acting on the payload.

### Governance and observability

The GA release adds connector RBAC, audit logs, and runtime observability.
Documented event categories cover successful token requests, completed
authorizations, token revocations, inbound triggers, and forwarded triggers.
Stable identifiers correlate tokens, authorizations, token groups, and trigger
deliveries.

These logs describe credential lifecycle activity. They are not a complete log
of every downstream provider API operation when application code calls the
provider directly.

## Marketing claim versus documented boundary

| Blog framing | Documentation detail | Design consequence for TrogonAI |
| --- | --- | --- |
| Applications do not store credentials | Connect stores durable grants, while the SDK receives and caches provider access tokens | Say "no durable provider credential in the application," not "no credentials" |
| Tokens are short-lived | OAuth and federated providers can issue short-lived tokens; API-key connectors start from a static key | Model the delivery mode and residual risk per provider |
| Tokens are scoped to the task | The caller supplies provider scopes and resources; no task identifier or single-use binding is documented | Derive provider authority from the admitted Session and tool operation server-side |
| Rotation disappears | OAuth refresh can be automatic, but customer-managed client secrets and static keys still rotate | Keep root-credential lifecycle, reconciliation, and provider-side rotation |
| One command revokes access | Provider invalidation depends on provider support; existing tokens may live until expiry | Represent local revocation and provider revocation as separate outcomes |
| The request needs no additional secret | This is true on Vercel with deployment OIDC; external runtimes use a Vercel access token | Prefer workload identity and make standing-token fallback explicit |
| Environment attachment isolates access | A project link gates issuance, but linked environments may share an installation | Use separate provider grants where provider-level isolation is required |
| The app has no webhook signing secret | Connect verifies the provider signature, but the receiver still verifies Connect's attestation | Keep an authenticated, replay-aware internal event boundary |

## Threat model and limits to retain

### Root credentials still exist

OAuth refresh tokens, client secrets, webhook secrets, private keys, and static
API keys move into the broker. They do not disappear. The broker becomes a
tier-zero service whose compromise can expose many provider grants.

TrogonAI therefore still needs OpenBao, narrow service identities, dual audit,
backup and restore discipline, break-glass controls, and customer-controlled
custody options where required.

### Provider capability is the ceiling

Scope precision, resource restriction, token TTL, refresh, revocation,
installations, and user delegation vary by provider. A broker cannot add a
provider-enforced property the provider does not support.

The product must not make a universal "short-lived and scoped" promise over a
static API key. That mode should be labeled as a weaker compatibility path.

### Returned credentials remain replayable

`getToken` returns a raw provider credential to server-side code. For OAuth and
federated connections this is normally a bearer access token. For an API-key
connection it can be the standing key itself. Compromised runtime code can copy
and replay the credential for its provider-defined lifetime. Connect does not
document a task-bound nonce, proof-of-possession key, or single-use provider
credential.

TrogonAI's draft sender-constrained internal grants and proxy-side provider use
are stronger. They should be preserved.

### Caller choice can widen authority

Connect accepts scopes, resources, subject, and installation in a runtime
request. A compromised application can ask for any combination allowed by the
connector and provider grant. Connect does not infer the least authority from
an agent plan or tool definition.

TrogonAI should not let native agent code submit arbitrary provider scope
strings, wildcard installations, audiences, or user identifiers. A versioned
provider driver should map an admitted typed operation to the exact provider
request within a platform policy ceiling.

### Workload identity is not user delegation

Authenticating the service does not authorize it to act as a particular person.
A user-subject request needs a separately recorded user consent and a verified
mapping to the provider subject. This agrees with draft ADR#0053, which says
external workload federation does not imply delegated user authority.

### Issuance controls do not recall issued tokens

Removing a project link, disabling an agent, or revoking a local binding can
stop future issuance immediately. Those actions do not necessarily invalidate a
token already accepted by a provider. Offboarding is complete only after both
the local authorization plane and provider plane reach the required state or
the residual token lifetime expires.

### The broker is a hot-path dependency

Token exchange and refresh add network latency and a new availability
dependency. Caching reduces load but creates a stale-authorization window. A
production design needs bounded caches, refresh-storm control, fail-closed
behavior, provider-specific retry rules, and a measurable residual exposure
window.

### Runtime security remains necessary

Credential brokering does not solve prompt injection, malicious tool selection,
business ownership checks, approvals, confused-deputy requests, SSRF, or
provider endpoint validation. Those remain admission and egress-policy
responsibilities.

## Current TrogonAI position

### Repository-evidenced foundation

| Area | Current evidence | State |
| --- | --- | --- |
| Secret custody direction | [ADR#0023](./docs/adr/0023-secret-management-and-key-custody-direction.md) selects OpenBao behind a platform secrets service and opaque `SecretRef` handles | Accepted direction |
| Typed secret operations | `secret_store/traits.rs` separates put, get, rotate, revoke, metadata, and destroy | Implemented in `trogon-gateway` |
| OpenBao storage | `openbao_secret_store.rs` implements KV v2 writes, reads, versions, metadata, revocation, and destruction | Implemented prototype |
| Credential lifecycle | Event-sourced commands, sagas, snapshots, runtime projection, recovery, and idempotency exist | Implemented gateway slice |
| Metadata authority | [ADR#0047](./docs/adr/0047-event-sourced-credential-metadata.md) makes the event stream the first-version source of truth | Accepted |
| Plaintext exposure | [ADR#0048](./docs/adr/0048-one-time-plaintext-exposure.md) permits one-time direct responses and metadata-only replay | Accepted |
| Runtime cache | `RuntimeCredentialCache` has a 300-second TTL and subtracts up to 30 seconds of deterministic jitter | Implemented |
| Delivery policy | Host, runtime-service, injection-location, and cache-TTL policy types exist and are checked before lookup | Implemented but not populated by production state |
| Revocation objective | [ADR#0049](./docs/adr/0049-revocation-latency-target.md) sets a 5-second normal-operation p99 target and a TTL backstop | Accepted target, alert not deployed |
| OpenBao production seal | [ADR#0052](./docs/adr/0052-cloud-kms-production-seal.md) requires cloud KMS auto-unseal in production | Accepted direction |
| Customer-controlled key routing | [ADR#0030](./docs/adr/0030-customer-controlled-key-backend-routing.md) defines optional customer-controlled KEK backends behind `KeyManagement`, not `SecretStore` | Draft |
| Key-custody tiers | [ADR#0033](./docs/adr/0033-two-tier-key-custody-product-model.md) distinguishes platform-managed and customer-managed business-key custody | Draft |
| Model access broker | [ADR#0032](./docs/adr/0032-model-route-and-credential-binding.md) defines connection, credential binding, delegated identity, workload identity, attempt-scoped grant, and session proxy | Draft and model-specific |
| External federation | [ADR#0053](./docs/adr/0053-external-oidc-federation-surface.md) defines a short-lived external workload assertion boundary | Draft |
| Extraction seam | [ADR#0061](./docs/adr/0061-credential-platform-extraction-boundary.md) moves OpenBao and lifecycle machinery behind the trait boundary | Proposed and blocked on four decisions |

### Current implementation gaps

1. The gateway is still the OpenBao client and reads `OPENBAO_TOKEN` from the
   environment. This is the direct-read shape accepted ADR#0023 supersedes.
2. The internal `/-/credentials` API uses one shared admin token and trusts a
   caller-supplied owner identifier. It does not derive project ownership from
   an authenticated principal.
3. OpenBao policy files exist, but no production service identity is bound to
   them. The gateway's token can reach operations broader than runtime read.
4. `RuntimeDeliveryPolicy` defaults are permissive for hosts and runtime
   services because the management API and credential event stream do not
   populate the policy.
5. `RuntimeCredentialResolver::resolve_plaintext_for` returns a raw
   `SecretString` to its caller. There is no general provider-token exchange or
   trusted egress-injection path.
6. No generic connector, installation, provider authorization, delegated OAuth
   refresh, remote MCP authorization, or provider revocation lifecycle exists
   in code.
7. Runtime cache entries expire, but the cache has no entry or byte limit and
   is keyed by `CredentialRef`, not a future full token-request context.
8. `SecretString` is an `Arc<str>` without zeroization. Clones can extend the
   lifetime of plaintext in process memory.
9. Credential access audit, an OpenBao audit device, deployed alerts, and
   anomaly detection are not complete.
10. Recovery covers pending activation, not the full lifecycle. `WriteFailed`
    is terminal and currently lacks an operator-visible recovery path.
11. There is no public credential management API, UI, or implemented API-key
    platform.
12. There is no `KeyManagement` implementation. Draft ADR#0030 and ADR#0033
    address customer-controlled KEK routing and custody tiers, not provider
    secret storage or runtime token exchange.
13. Existing Vercel notes in `SECRET_STORE.md` focus on environment-variable
    delivery and do not cover the newer token-broker model.

### Documentation conflicts to resolve

- `docs/architecture/secret-management.md` says the prototype is absent, which
  is no longer true in this checkout.
- `CREDENTIAL_PLATFORM_SPEC.md` assigns lifecycle state to an application
  database in one section, while accepted ADR#0047 selects the event stream as
  the first-version source of truth.
- The architecture says consumers hold plaintext only for an in-flight
  operation, while the gateway implements an ambient plaintext cache and a
  long-lived Discord connection. Proposed ADR#0061 already identifies this
  conflict.
- ADR#0049 describes the cache backstop as 300 seconds plus up to 30 seconds of
  jitter, for a 330-second maximum. The implementation subtracts jitter from
  the 300-second TTL, so entries expire between roughly 270 and 300 seconds.
  The accepted ADR and code need one authoritative interpretation.
- Internal credential IDs still encode an `openbao:` prefix. Public responses
  avoid that provider coupling, but the internal migration question remains.

## What to adopt, adapt, and avoid

### Adopt

- Workload identity as the normal broker credential.
- A first-class connection and installation model separate from secret bytes.
- App and delegated-user subjects as distinct authorization modes.
- Runtime token exchange and automatic refresh where providers support them.
- Per-request provider authority and explicit resource restriction.
- Project and environment attachment as issuance policy.
- Provider capability metadata for scope precision, TTL, refresh, and
  revocation.
- Typed recoverable errors for missing installation, missing consent, revoked
  grant, disabled environment, and provider failure.
- Correlated issuance, authorization, revocation, and trigger audit events.
- Central provider-webhook verification followed by an authenticated internal
  event.

### Adapt to TrogonAI

- Reuse the `ModelProviderConnection`, `CredentialBinding`, and short-lived
  grant concepts from draft ADR#0032 as the pattern. Introduce a general
  connection model through an ADR instead of copying Vercel names directly.
- Authenticate the broker over TrogonAI's workload and NATS identity boundary,
  not Vercel-specific OIDC.
- Bind access to project, principal, AgentRevision, Session, ExecutionAttempt,
  tool or channel operation, destination, and purpose when those contexts
  exist.
- Map typed tool operations to provider-native scopes inside a versioned driver.
  Do not expose provider scope strings as the platform's authorization
  vocabulary.
- Use OpenBao for durable refresh material and static roots, while keeping
  temporary provider tokens memory-only.
- Record both local revocation state and provider revocation outcome.
- Keep environment as an attribute, but require separate provider grants when
  environment-level provider isolation is requested.

### Do not copy

- Do not return raw provider tokens to native agent code, prompts, tool
  arguments, tool results, transcripts, checkpoints, or crash artifacts.
- Do not permit `scopes: ['*']`, wildcard installations, arbitrary audiences,
  or caller-selected provider subjects from an agent-controlled request.
- Do not describe a static API-key connection as short-lived.
- Do not treat project-link removal as recall of an already-issued token.
- Do not make a Vercel access-token-style standing credential the normal
  self-hosted bootstrap path.
- Do not create one generic provider interface made of unvalidated strings.
- Do not assume connector observability replaces downstream action audit.
- Do not place every provider call behind an HTTP proxy when an operation-only
  cryptographic check or a provider workload identity is the narrower boundary.

## Recommended target boundary

The exact names require an ADR. The important separation is:

```text
SecretStore
  -> durable retrievable material
  -> refresh tokens, client secrets, static API keys, bot tokens

ProviderAccessBroker
  -> authorizes one provider-access request
  -> exchanges or refreshes durable material
  -> returns only to a trusted adapter, or performs the provider call itself

KeyManagement
  -> non-exportable sign, verify, encrypt, decrypt, wrap, and rewrap operations

PlatformCallerAuthentication
  -> verifier-only bearer keys or signed proof-of-possession
  -> never a path to raw provider credential material
```

The broker may be hosted by the platform secrets service, but it should remain
a separate typed domain port. Adding token exchange, provider authorization,
and egress policy to a generic `SecretStoreGet` method would erase the security
decision that makes Connect useful.

### Candidate control-plane records

These are decision inputs, not approved schema:

```text
ProviderConnection
  project
  environment binding
  provider security and billing boundary
  provider driver version and configuration digest
  lifecycle state

CredentialBinding
  exactly one authentication mode
    WorkloadIdentity | DelegatedIdentity | StoredCredential |
    MutualTlsIdentity | NoCredential
  durable SecretRef only where material must be stored
  lifecycle state and continuity fingerprint

ProviderInstallation
  one external tenant or account grant
  provider subject and tenant fingerprints
  granted authority and consent state

ProviderAuthorization
  app or person-linked subject
  granted authority, consent version, expiry, and refresh capability

ProviderAccessGrant
  project + principal + Session + ExecutionAttempt + operation
  connection + binding + installation + provider subject
  resource and authority ceiling
  audience + confirmation key + expiry + lifecycle state
```

The model-specific names in draft ADR#0032 should remain model-specific. A
general connection model should be factored underneath or beside them only
after the relationship is decided, so model routing does not drift from tool
and channel access.

### Runtime outbound flow

```text
native agent requests an admitted typed tool operation
  -> trusted supervisor or tool executor authenticates to the broker
  -> broker derives project, principal, Session, attempt, tool, and purpose
  -> broker loads the pinned connection, binding, installation, and policy
  -> provider driver derives exact scopes, resources, audience, and endpoint
  -> broker obtains a temporary provider credential
  -> trusted adapter applies the credential and calls the provider
  -> native agent receives only the typed provider result
  -> audit joins admission, broker issuance, and provider request outcome
```

The native agent never receives the internal grant, refresh material, provider
token, or credential injection instructions.

### Provider access modes

| Mode | Preferred handling | Example |
| --- | --- | --- |
| Workload federation | Exchange deployment-attested identity for a short-lived provider credential | Cloud model or data service role |
| Delegated OAuth | Store refresh material in OpenBao and mint access tokens for a named authorized person | User-authorized GitHub or Google action |
| Application OAuth | Store client material in OpenBao and mint app access tokens | Service-level CRM integration |
| Static provider API key | Keep the key in OpenBao and apply it inside a trusted egress adapter; raw delivery is an explicit degraded exception | Provider with no exchange protocol |
| Operation-only secret | Perform verify or sign inside the secrets or key service without releasing material | Webhook HMAC verification |
| Long-lived transport token | Confine the token to one typed adapter, record the exception, and revalidate on reconnect | Discord or Slack socket connection |

### Token cache rules

A temporary-token cache should:

- be keyed by connection, binding version, installation, subject, scopes,
  resources, audience, environment, and relevant policy digest;
- have configurable entry and byte limits;
- expire at the earliest of provider expiry minus skew, policy TTL, grant
  expiry, Session or attempt termination, and binding revocation;
- never cache an external federation assertion used to obtain the provider
  token;
- invalidate on rotation, revocation, unbinding, consent change, provider
  rejection, and policy change;
- prevent refresh stampedes with single-flight behavior and jitter;
- zeroize uniquely owned plaintext buffers where practical; and
- expose hit, miss, refresh, expiry, eviction, and stale-denial metrics without
  credential values.

### Provider capability matrix

Every provider driver needs a tested capability record covering:

- supported authentication modes;
- app, user, and federated subjects;
- installation or tenant behavior;
- scope and resource precision;
- audience and rich authorization support;
- access-token TTL and refresh-token behavior;
- refresh-token rotation;
- provider revocation support and expected residual lifetime;
- consent and reconsent rules;
- webhook verification and trigger support;
- subject or account continuity probes; and
- whether a safe non-billable validation request exists.

The product must derive its claims and UI from this matrix. Unsupported
properties are explicit, never silently simulated.

## Prioritized improvement plan

Every item includes a concrete context so the intended security boundary is
testable.

### P0: close existing safety gaps before expanding the provider catalog

1. **Extract the secrets service, then replace standing OpenBao credentials with
   scoped, deployment-attested identities.** The secrets service remains the
   only OpenBao client and obtains operation-scoped credentials under the five
   existing policies. The gateway receives no OpenBao role. If extraction cannot
   happen immediately, narrowing the gateway token is temporary containment,
   not the target architecture. Example: the gateway authenticates to the
   secrets service to verify an active GitHub webhook and has no credential that
   can read, rotate, or destroy OpenBao material directly.
2. **Ratify ADR#0061 with provider-access operations included in Q1.** The
   boundary must cover value resolution, operation-only verification, OAuth
   exchange or refresh, workload federation, and trusted credential injection.
   Example: a Slack post uses an ephemeral app token without exposing the stored
   refresh grant.
3. **Derive project ownership from the authenticated principal.** Remove the
   shared-token plus caller-supplied-owner trust model. Example: a project-A
   operator cannot name project B in a create or resolve request.
4. **Persist and populate delivery policy.** Carry policy through commands,
   events, projections, and management APIs, with deny-by-default behavior for
   newly managed outbound connections. Example: a GitHub credential admitted
   for one repository and `api.github.com` is denied for another host or an
   unidentified runtime service.
5. **Define dual-plane revocation.** Stop future issuance locally, invalidate
   every cache replica, attempt provider revocation, record the outcome, and
   measure remaining token lifetime. Example: removing a user's Slack consent
   produces `local_revoked` immediately and `provider_revocation_unsupported`
   until the last token expires.
6. **Bound and harden plaintext memory.** Add cache entry and byte limits,
   configurable TTL, per-credential invalidation, and zeroizing secret
   containers where ownership permits it. Example: a multi-tenant gateway
   cannot accumulate one plaintext token per integration without a size bound.
7. **Complete production audit and alert prerequisites.** Provision the OpenBao
   audit device, join business and provider audit with correlation IDs, deploy
   credential alerts, and expose the incomplete-provision backlog. Example: an
   operator can identify a `WriteFailed` credential without reading secret
   material.

### P1: build the general provider-access broker

1. **Generalize the draft ADR#0032 access pattern.** Reuse its connection,
   binding, workload identity, delegated identity, short-lived grant, and
   proxy-side access principles for tools, channels, and MCP. Example: a Session
   tool call receives a typed issue result, not a GitHub token.
2. **Implement provider installation and authorization lifecycles.** Model
   consent, reconsent, refresh rotation, uninstall, subject continuity, and
   external tenant identity. Example: adding a new Microsoft permission does
   not affect an existing user grant until reauthorization succeeds.
3. **Derive provider authority from admitted operations.** Keep platform
   permissions separate from provider-native scopes, and map them in a versioned
   driver. Example: `ReviewRepository` maps to read-only repository authority;
   an agent cannot replace it with an organization-wide wildcard.
4. **Implement context-complete temporary-token caching.** Include subject,
   installation, resource, audience, scope, environment, and policy digest in
   the key. Example: a token minted for one user or repository can never satisfy
   another user's request through a cache collision.
5. **Add broker audit projections and drains.** Record successful and failed
   issuance, consent, refresh, revocation, cache decisions, and proxied provider
   actions with stable correlation IDs. Example: one Session tool call can be
   traced from admission through token issuance to provider request ID without
   logging a token.
6. **Build and enforce the provider capability matrix.** Product behavior,
   error types, validation, and UI promises come from tested driver
   capabilities. Example: a static API-key provider displays a residual-risk
   warning instead of a short-lived-token claim.
7. **Make environment isolation explicit.** Permit a shared connection only
   when policy allows shared provider tenancy. Example: production and QA use
   separate Slack installations even though both belong to one project.
8. **Decide the inbound trigger boundary.** Compare secrets-service HMAC
   operations with gateway-local verification, then re-attest accepted events
   over the authenticated internal transport. Example: the Session receives a
   verified Slack event and correlation metadata, never the signing secret.

### P2: productize after the security boundary is proven

1. **Add curated managed and customer-managed connector flows.** Example: a
   managed GitHub application and a bring-your-own custom OAuth client use the
   same connection lifecycle but different custody ownership.
2. **Add OAuth-capable MCP connection discovery with strict metadata
   validation.** Example: an MCP server origin and discovered issuer are pinned
   and revalidated before a token is sent.
3. **Add dashboard, CLI, and API surfaces over the same resources.** Example:
   lists are metadata-only, secret input is write-only, and consent or
   installation recovery is a typed user-facing state.
4. **Add quotas, cost attribution, and broker capacity planning.** Example:
   token refresh storms and high-volume triggers have per-project limits and do
   not turn the broker into an unbounded shared bottleneck.

## Required security and conformance tests

- Cross-project, cross-environment, cross-installation, cross-user, and
  cross-Session access is denied before secret or cache lookup.
- An agent cannot widen scopes, select another subject, substitute an audience,
  or request a wildcard installation.
- Provider scopes and resources are derived from the admitted typed operation
  and stay within both the platform policy and provider grant.
- Durable refresh material, temporary tokens, internal grants, confirmation
  keys, and authorization headers never enter events, snapshots, NATS durable
  messages, databases, prompts, transcripts, logs, traces, metrics, errors, or
  crash artifacts.
- Native agent code cannot observe the provider token during a proxied tool or
  model call.
- Cache keys cannot alias across subjects, resources, installations, policy
  versions, or environments.
- Rotation, policy change, consent removal, Session termination, and revocation
  invalidate every affected cache replica.
- Provider revocation unsupported, rejected, failed, and outcome-unknown states
  are distinct and preserve the residual exposure window.
- A provider token already issued before local revocation remains visible as
  outstanding until provider revocation or expiry.
- Refresh-token rotation is atomic from the broker's point of view and has a
  recovery path for an outcome-unknown provider exchange.
- Workload identity proves the expected project and runtime; it never
  substitutes for user consent.
- Provider endpoint validation rejects redirects, DNS rebinding, metadata
  services, loopback, link-local, untrusted certificates, and undeclared hosts
  before a credential-bearing request.
- Invalid, replayed, misrouted, or oversized inbound trigger events are denied
  before business handling.
- Broker, OpenBao, provider, and event transport outages fail closed without
  silently selecting another credential, provider account, user, or route.
- Static API-key connectors never report short-lived or immediate-revocation
  guarantees they cannot enforce.

## Decisions this research should inform

### Proposed ADR#0061

Q1 currently asks whether the secrets-service surface is a value verb, an
operation verb, or both. The recommendation should remain "both" and become
more precise:

- raw value resolution is the compatibility path;
- operation-only verification or signing is preferred when the value need not
  leave custody;
- token exchange, refresh, and workload federation are first-class operations;
- trusted egress injection or proxying is preferred when an agent should not
  see even the temporary token; and
- provider-specific operations live behind typed drivers, not a stringly
  generic request.

### Draft ADR#0032

Keep its stronger properties:

- provider credentials stay out of agent and execution state;
- the native implementation never receives an upstream token;
- access grants are short-lived, attempt-scoped, sender-constrained, and bound
  to the immutable Session plan;
- the provider route and credential binding remain pinned; and
- revocation fails the next request closed without fallback.

Add a companion decision for how this pattern relates to non-model external
services. Do not silently broaden a model-specific ADR.

### Draft ADR#0053

Keep the separation between workload authentication and user delegation. Add
the provider-access broker as a concrete consumer of short-lived external
assertions, while retaining the stated fact that provider-issued tokens outlive
platform assertions and need their own revocation accounting.

### Credential platform specification

Extend the product model beyond vaults, credentials, and delivery rules to
include connection, installation, delegated authorization, provider capability,
and runtime access grant concepts. Preserve the accepted event-stream source of
truth and remove the conflicting application-database write-model language.

### Existing Vercel comparison

Rename or split the Vercel section in `SECRET_STORE.md`:

- Vercel Environment Variables remain a reference for secret management and
  deployment delivery.
- Vercel Connect is a different reference for workload identity, delegated
  consent, provider token exchange, runtime scoping, and trigger brokering.

Conflating them hides the new architectural layer this post introduces.

## Product and operational considerations

- Vercel Connect pricing is currently request-based. As published, Hobby
  includes 500 token requests and 1,000 triggers per month; Pro pricing is
  $3 per 1,000 token requests and $0.95 per 1,000 triggers; Enterprise pricing
  is custom. Updated beta-customer billing is scheduled for September 25,
  2026. These values are time-sensitive.
- Trigger deliveries count separately and may still be billed when no
  destination is configured. TrogonAI needs explicit cleanup and quota
  semantics even if self-hosted accounting differs.
- Provider availability and revocation behavior remain external dependencies.
- Product terms, provider terms, regulated-data restrictions, residency, and
  customer responsibility need review before adopting a managed broker.
- No public Connect documentation reviewed here establishes per-tenant
  encryption keys, customer-controlled key custody, HSM isolation, backup
  destruction semantics, or breach-cell isolation. TrogonAI's key-custody work
  remains an independent requirement.
- The Vercel CLI documentation still uses beta wording in places even though
  the product announcement says GA. Treat product status and individual tool
  status separately.

## Primary sources

- [The end of credential sprawl for agents](https://vercel.com/blog/the-end-of-credential-sprawl-for-agents)
- [Vercel Connect overview](https://vercel.com/docs/connect)
- [Connectors](https://vercel.com/docs/connect/concepts/connectors)
- [Installations](https://vercel.com/docs/connect/concepts/installations)
- [Tokens](https://vercel.com/docs/connect/concepts/tokens)
- [Authentication](https://vercel.com/docs/connect/concepts/authentication)
- [Project links](https://vercel.com/docs/connect/concepts/project-links)
- [Triggers](https://vercel.com/docs/connect/concepts/triggers)
- [Observability](https://vercel.com/docs/connect/observability)
- [TypeScript SDK reference](https://vercel.com/docs/connect/ts-sdk-reference)
- [REST API: get a Connect token](https://vercel.com/docs/rest-api/connect/get-a-connect-token)
- [REST API: create an authorization request](https://vercel.com/docs/rest-api/connect/create-a-connect-authorization-request)
- [Vercel OIDC federation](https://vercel.com/docs/oidc)
- [Pricing and limits](https://vercel.com/docs/connect/pricing)
- [Vercel Connect product terms](https://vercel.com/docs/connect/legal)

## Repository sources

- [ADR#0023: Secret Management and Key Custody](./docs/adr/0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](./docs/adr/0030-customer-controlled-key-backend-routing.md)
- [ADR#0032: Model Route and Credential Binding](./docs/adr/0032-model-route-and-credential-binding.md)
- [ADR#0033: Two-Tier Key Custody Product Model](./docs/adr/0033-two-tier-key-custody-product-model.md)
- [ADR#0046: Project-Anchored Resource Hierarchy](./docs/adr/0046-project-anchored-resource-hierarchy.md)
- [ADR#0047: Event-Sourced Credential Metadata](./docs/adr/0047-event-sourced-credential-metadata.md)
- [ADR#0048: One-Time Plaintext Exposure](./docs/adr/0048-one-time-plaintext-exposure.md)
- [ADR#0049: Revocation Latency Target](./docs/adr/0049-revocation-latency-target.md)
- [ADR#0052: Cloud KMS Production Seal](./docs/adr/0052-cloud-kms-production-seal.md)
- [ADR#0053: External OIDC Federation Surface](./docs/adr/0053-external-oidc-federation-surface.md)
- [ADR#0061: Credential Platform Extraction Boundary](./docs/adr/0061-credential-platform-extraction-boundary.md)
- [Credential platform specification](./CREDENTIAL_PLATFORM_SPEC.md)
- [Secret store context](./SECRET_STORE.md)
- [`SecretStore` traits](./rsworkspace/crates/platform/trogon-gateway/src/secret_store/traits.rs)
- [OpenBao adapter](./rsworkspace/crates/platform/trogon-gateway/src/secret_store/openbao_secret_store.rs)
- [Runtime credential projection and cache](./rsworkspace/crates/platform/trogon-gateway/src/credential/processor/runtime_projection.rs)
- [Gateway OpenBao bootstrap](./rsworkspace/crates/platform/trogon-gateway/src/main.rs)
- [`SecretString`](./rsworkspace/crates/platform/trogon-std/src/secret_string.rs)
