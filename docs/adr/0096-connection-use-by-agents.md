---
number: "0096"
slug: connection-use-by-agents
status: draft
date: 2026-11-03
---

# ADR#0096: An Agent Uses a Connection Through an Authorized HTTP Proxy That Injects Credentials Held by Custody

> Contract status: the protobuf drafts now define the authenticated HTTPS
> relay profile, binding, use modes and safe audit metadata described here.
> The earlier typed `access.InvokeOperation` contract is removed. These are
> schema and design contracts; no proxy runtime or deployed enforcement is
> established by this work.

## Context

[ADR#0063](./0063-agent-connection-declarations.md) lets an AgentRevision
state which connections it needs. [ADR#0092](./0092-third-party-connection-model.md)
defines the connection resource, [ADR#0093](./0093-connection-ownership-and-delegation.md)
its owner and permissions, [ADR#0094](./0094-oauth-authorization-protocol.md)
consent, and [ADR#0095](./0095-connection-credential-custody-and-refresh.md)
credential storage and refresh. This ADR defines how an admitted Session uses
that authority without receiving the provider credential.

The provider call is an HTTP proxy path. Agent code constructs an ordinary
provider HTTP request. An authorized proxy checks the connection and grant,
obtains the credential from custody, injects it, forwards the request and
returns the HTTP response. Protobufs govern connection configuration,
authorization and lifecycle; they do not replace the provider's HTTP API with
an RPC catalog.

The earlier draft required a typed operation in a ConnectorRelease for every
provider call, with input and output schemas and `InvokeOperation` dispatch.
That requirement is withdrawn. A connector still needs provider-specific
authentication metadata and identity-probe behavior, but connecting a provider
does not require modeling its entire API.

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) keeps OpenBao
behind the secrets service. The proxy parses untrusted HTTP outside that
service. Shared provider apps also require fair budgets, and credential use
requires durable audit acceptance before dispatch.

See [Third-Party Connections](../architecture/third-party-connections.md) for
the architecture explanation and contract boundary.

## Decision

The call path is:

```text
Agent HTTP request -> authorized connection HTTP proxy -> credential injection -> provider
Agent HTTP response <- authorized connection HTTP proxy <- provider HTTP response
                                      |
                         custody resolves and refreshes
```

The agent receives neither the provider token nor refresh credentials. The
proxy can use an access credential only for the connection authorized by a live
grant. Custody owns refresh, so the proxy never needs a refresh token.

### 1. Admission resolves declarations to connections, and the plan records references only

`admission.ResolveConnectionDeclarations` runs at Session creation. A
selector names a provider, which is the `ConnectorId`, plus labels. Selection
precedes permission evaluation: policy can refuse a selected connection but
cannot steer selection toward another one
([ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)).
The result is one Active connection or a typed failure.

| Failure | Meaning |
| --- | --- |
| `NO_CONNECTION` | No match; an optional declaration records absence |
| `AMBIGUOUS` | More than one match; fails even for an optional declaration |
| `RECONNECT_REQUIRED` | The selected connection needs owner consent again |
| `RELEASE_BLOCKED` | The pinned release denies live use |
| `NOT_PERMITTED` | No live permit or required personal delegation |

A resolved use records the binding name, `ConnectionRef`,
`authorization_generation`, the admitted authority bound and the
`PolicyRuleRef` that admitted it in the immutable SessionExecutionPlan. It
contains no credential, `SecretRef` or live grant. Provider scope and any
expressly supported request restrictions bound the use; a complete list of
provider operation refs is not required.

Policy is read live when a request reaches the proxy. The plan and grant do not
freeze a permit's current ceiling. Scope reduction lowers live authority on a
connection that can remain Active, as described in
[ADR#0094](./0094-oauth-authorization-protocol.md).

### 2. A ConnectionGrant binds proxy use to an execution attempt

A grant authorizes use of one planned connection by one attempt. Its lifecycle
remains Prepared, Activated, then Revoked or Expired, following
[ADR#0032](./0032-model-route-and-credential-binding.md). It binds the Session,
execution attempt, AgentRevision, plan digest, connection reference and
authorization generation, confirmation key, audience, admitting permit and
expiry. Its audience identifies the authorized proxy use path.

The grant's immutable `resource_parent` is the Session's project. The pinned
ConnectionRef carries its own custody parent, which may differ. The [resource-parent
reference](../reference/connector-resource-parents.md) defines both names and
requires authoritative Session-project checks rather than equating these parents.

The supervisor holds the confirmation key outside agent code. A leaked grant
identifier grants nothing by itself. Proxy admission must authenticate the
caller and bind the request to the authorized attempt, connection and
destination. `ProxyBinding` fixes the binding id, configured HTTPS relay origin,
exact upstream host/port and optional credential role. Only an anonymous
connection omits the role. Admission selects this binding and the grant copies
it unchanged. The supervisor proves each HTTP request using the signed relay
profile in [Connection HTTP Proxy](../reference/connection-http-proxy.md).

- Grants activate only after the execution attempt is Ready. Activation failure
  revokes its grants and fails the attempt; attempt end revokes them too.
- Expiry is at most five minutes, renewable only by the supervisor's bounded
  renewal capability and never beyond attempt end.
- Live connection and permit checks enforce revocation within the bounded-live
  rule; a grant's remaining lifetime cannot override them.
- A child Session runs its own admission and receives its own grants. It cannot
  reuse a parent's authority.
- The grant issuer has no provider network path. The component forwarding a
  provider request cannot mint authority for itself.
- The grant stream lives with the Session, in its project, and uses the
  sensitive stream classification of
  [ADR#0092](./0092-third-party-connection-model.md).

### 3. Connection permissions authorize proxy use, and absence is denial

`policy/connection_permissions` retains initialize, set and clear commands and
events attached to a `PolicySubject`. A permit names an exact connection or
connections under a custody project. Person-owned connections still require
the owner's explicit permit on an exact connection, as
[ADR#0093](./0093-connection-ownership-and-delegation.md) specifies. A node
administrator cannot create that personal delegation through a subtree rule.

Normal connection use is proxy use. A permit can narrow the provider authority
and any request restrictions the proxy actually supports. MCP tool allowlists
remain explicit protocol-aware restrictions. A generic HTTP proxy does not
establish operation-level or field-level authorization merely by forwarding a
request; such restrictions require a separately specified enforcement rule.

The wire modes are `HTTP_PROXY`, `MCP_SESSION` and optional future
`TOKEN_ISSUANCE`. An empty permit mode set means HTTP proxy use only; MCP must
be named. The retired `INVOKE_OPERATION` number and name are reserved. Grant
audiences must match their mode. Personal delegations may grant HTTP proxy or
MCP session use, never token issuance.

HTTP restrictions match exact host/port, an allowed method and a literal path
prefix at a segment boundary. Rules within a set are alternatives; release,
planned-grant and live-permit sets intersect. Scope allowlists require the
actual credential scope set to be a subset. They deny a broadly scoped
credential rather than pretending that the proxy attenuates it. Unsupported
restrictions are refused before custody or dispatch.

The proxy checks the logical upstream against live Session egress policy in
addition to the connection permit. The relay origin is configured internal
routing, not permission to escape upstream restrictions. Neither a generic
egress role nor knowledge of a binding id grants custody access.

### 4. The proxy forwards HTTP requests under live authorization

The proxy processes each request in this order:

1. Authenticate the caller and validate the Activated, unexpired grant against
   the Session, attempt, plan digest, connection and request binding.
1. Check live connection, project, app and release standing, equal
   `authorization_generation`, the absence of an Agent hold, and a permit that
   still allows this use. Personal delegation also requires the owner's
   continuing access to custody.
1. Validate destination, request framing and the configured HTTP restrictions
   and resource limits. Rejection happens before credential resolution.
1. Consume the applicable budgets.
1. Make the acceptance audit fact durable.
1. Resolve the access credential through custody, which performs refresh when
   needed under [ADR#0095](./0095-connection-credential-custody-and-refresh.md).
1. Inject the credential using the connection's authorized authentication
   binding and forward to the validated provider destination.
1. Apply response bounds and credential protections, append completion metadata
   and return the provider's HTTP response.

The forwarded request retains its HTTP method, path, permitted headers and
body. The provider response retains its HTTP status, permitted headers and
body. These are not encoded as `google.protobuf.Any` operation input/output,
nor translated through a required provider-specific output schema.

The HTTP relay writes `Trogon-Connection-Response: upstream` for a preserved
provider response, after stripping all upstream `Trogon-Connection-*` headers.
Its own refusal writes `proxy` plus a base64url `ConnectionProxyFailure` in
`Trogon-Connection-Failure`, with an empty body and `Cache-Control: no-store`.
The [relay reference](../reference/connection-http-proxy.md) defines statuses,
signed request metadata and retry semantics. Internal typed outcomes remain
audit facts, not a provider response envelope.

Admission, proxy use and trusted MCP use read projections whose staleness is
measured from projector heartbeats. They refuse new work after a configured
bound of at most 330 seconds, the cache TTL plus jitter of
[ADR#0049](./0049-revocation-latency-target.md). An unavailable or stale policy
source is not a reason to use an older snapshot. A request already dispatched
finishes under its deadline; the next request is checked again.

A refresh or provider failure never selects another connection, app or release.
The proxy must not automatically replay a request that might have had an
upstream effect. An attributable authentication rejection can trigger custody health/refresh
work for a later request. The v1 relay never automatically replays the accepted
HTTP request after refresh. A lost response remains an unknown outcome. The
[receipt recovery contract](../reference/connector-relay-recovery.md) fixes the
nonce, audit, send-intent and restoration boundaries.

The trusted proxy and MCP caller report provider authentication observations
through `ReportConnectionHealth`. A definitive dead credential leads to
ReconnectRequired. An attributable insufficient-scope response leaves the
connection Active with `SCOPE_STEP_UP`, and a reduced grant remains bounded by
the scopes actually held. Generic HTTP status alone does not prove the full
provider-specific meaning of a response.

### 5. Credential injection runs outside custody

The provider-facing proxy holds access credentials only in bounded, zeroized
memory. It has no OpenBao client or token, refresh token, provider app secret
or app private key. `ResolveForTrustedUse` remains an authenticated internal
custody operation under a live grant, or under a narrowly authorized attempt
for an identity probe. Its result is sealed to the authorized trusted caller's
xkey. Custody authorizes from its own state and refuses when that state cannot
be read.

The proxy is the trusted provider-facing role for ordinary HTTP calls. An
arbitrary egress process does not gain resolve privileges merely because it
forwards traffic. How this role reuses, integrates with, or is isolated from
[agent egress proxy proposal](https://github.com/TrogonStack/trogonai/pull/746)'s egress proxy remains a deployment and identity decision. This ADR
does not require a new service, queue group or independent proxy product.

Access credentials never enter agent-visible configuration, provider request
logs, traces, event streams or persistent caches. Cached access is bounded by
`valid_until` and the revocation target of
[ADR#0049](./0049-revocation-latency-target.md), evicted by invalidation and
authorization-generation changes. Only custody calls token, registration and
revocation endpoints or signs client assertions.

Identity probing remains provider-specific trusted behavior. The probe uses a
staged credential under the authorization attempt and extracts only the
account fields permitted by the release. It can reuse the authorized HTTP
forwarding foundation outside custody; it does not require a public operation
catalog for every agent request. Custody parses token and registration
responses, while the identity probe parses the limited provider API answer.

### 6. Forwarded HTTP is bounded without a mandatory provider operation schema

Request and response bounds apply before or during forwarding. Releases may
lower platform limits, not raise them past the platform ceiling. The
[finite HTTP profile](../reference/connector-http-transport.md) fixes hard
ceilings, identity-only content encoding, bodyless response handling and explicit
rejection of ranges, SSE, WebSocket and protocol upgrades. The
[binding contract](../reference/connector-binding-integration.md) separates API
origins per credential role from OAuth and discovery egress.

| Limit | Initial default | Notes |
| --- | --- | --- |
| Request body | 256 KiB | Reject before resolving a credential when known to exceed the cap |
| Response body | 2 MiB | Abort a read that exceeds the cap |
| Connect timeout | 5 s | |
| Total deadline | 30 s, bounded by grant expiry | Finite buffered HTTP only; streaming requires a separate profile |
| Redirects | No automatic following | A provider redirect is returned subject to header and destination protections |
| In flight per grant | 4 | Bounds concurrent use |

Destination validation uses exact release-approved hosts, or the acknowledged
hosts of a pinned MCP discovery snapshot, with the address and redirect rules
of [ADR#0094](./0094-oauth-authorization-protocol.md). Credentials cannot move
to a new origin through a caller-supplied destination, redirect or mismatched
Host header. The proxy constructs destination authority and framing rather
than trusting conflicting hop-by-hop or proxy-control headers.

Ordinary provider headers are forwarded under the HTTP header policy.
Credential-bearing headers such as Authorization are owned by the connection's
authentication binding: the proxy replaces or rejects caller-supplied values
and never forwards proxy authentication to the provider. Cookie handling and
any provider-specific authentication placement must be explicit in that
binding. Agent code cannot override the credential selection or insert a
SecretRef into a destination or request.

The proxy does not need a per-provider request/response protobuf, an exhaustive
endpoint catalog or a mandatory GraphQL parser. Passing HTTP does not establish
that a GraphQL mutation is a read-only operation. If policy promises
body-sensitive restrictions, an explicitly defined protocol-aware enforcement
mechanism must prove them before that policy is enabled.

Responses remain provider HTTP content, treated as untrusted. Credential-bearing
headers, cookie handling and credential-bearing redirects are governed by the
proxy's response policy; the injected credential must never be returned to
agent code. The relay buffers the bounded response and refuses a detected
credential echo with `CREDENTIAL_RESPONSE_REJECTED`, removes Set-Cookie and
unsafe Location headers, and strips its control/proof headers. Encoded or
transformed echoes are not claimed safe by a generic schema; a provider that
needs stronger handling requires a reviewed trusted filter. Bodies and credential-bearing URLs do not enter audit records or
error diagnostics.

### 7. Trusted MCP use retains its protocol-aware session boundary

An MCP connection carries the same connection/grant checks, but MCP sessions
also have protocol rules beyond ordinary HTTP forwarding. The trusted MCP
caller holds the upstream session, authenticates requests for the canonical
resource, refreshes through custody, and enforces any MCP tool allowlist on
both listing and calls. A generic HTTP proxy by itself does not filter
`tools/list` or understand a tool's authority.

The upstream session closes on connection revoke, destroyed custody key,
removal of the permit or personal delegation, an Agent hold, or grant
revocation or expiry. No token, mesh credential or token issued for another
resource passes through to agent code. Authentication and scope step-up follow
[ADR#0094](./0094-oauth-authorization-protocol.md), and the proxy request bounds
apply where compatible with the explicitly bounded MCP stream.

### 8. Budgets follow provider quota scope and divide shared app capacity fairly

A connector describes the provider's quota scope and response header metadata:
per credential, app or account. This metadata does not require operation
schemas. Counters distinguish connection, tenant share and provider app, using
the revision-checked counter approach of
[ADR#0077](./0077-api-key-authorization-model-and-rate-limits.md).

- A shared platform app has configured tenant shares and a total budget below
  the provider's limit. The first milestone does not lend idle capacity.
- Provider throttling updates the matching app or connection budget across
  replicas. Proxy-generated throttling remains distinguishable from an
  upstream HTTP response, with a bounded retry delay.
- Refresh has its own concurrency and fairness budget under
  [ADR#0095](./0095-connection-credential-custody-and-refresh.md). A live access
  credential is usable without a token call; an exhausted refresh budget
  affects calls that need refresh.
- Counter-store failure follows the existing split in
  [ADR#0077](./0077-api-key-authorization-model-and-rate-limits.md): shared
  platform-app protection fails closed, while connection-local fairness can
  fail open with a distinct metric. That exception does not relax connection,
  custody or audit authorization.

### 9. Durable audit acceptance precedes credential resolution and dispatch

Each forwarded request has an acceptance fact and completion fact on the
sensitive connection audit stream. The proxy appends acceptance with
acknowledgement after authorization and budget checks, before resolving any
credential or sending the provider request. Failure refuses dispatch.

A completion append failure cannot undo a provider call. A replicated, body-free
receipt journal records send intent and the terminal completion outbox before a
provider response is returned. Stable event identities make audit publication
retryable without repeating the provider call. An in-memory queue is only an
optimization. Reserve outbox capacity before admission; exhaustion or an aged
backlog refuses new work while existing outcomes drain. Acceptance without a
recoverable completion is reconciled as unknown, never resent. The
[recovery contract](../reference/connector-relay-recovery.md) specifies crash
cases, renewal, pruning and restore fencing. Export sinks consume acknowledged
facts asynchronously and do not independently gate forwarding.

Facts carry safe correlation and authority metadata: Session, attempt, plan,
connection/grant references, authorization generation, evaluated policy refs,
delegating owner where applicable, approved destination identity, result
category, status class, sizes, timings and safe provider request identifiers.
The proxy passes the audit correlation into custody resolution so both records
join. A required provider operation ref or typed API input is not needed for
that correlation.

No token, secret, Authorization header, request or response body, query values,
GraphQL variables or provider error body is retained. HTTP paths and headers
can contain personal or secret material and are not copied wholesale into
facts or diagnostics. An opaque request correlation identifier is not
permission to persist the request.

### 10. The baseline is an authenticated HTTPS relay

Phase 1 uses a project-owned Linear connection on the platform app. The
supervisor routes HTTP to the grant's fixed relay origin, at
`/v1/connection-proxy` followed by the original provider path and query.
`ProxyRequestContext` in `Trogon-Connection-Context` identifies the grant and
binding without carrying a provider credential. The relay strips the fixed
prefix, selects only the binding's immutable upstream, checks authority and
injects the credential. A provider endpoint needs no protobuf RPC.

The supervisor holds the confirmation key and adds RFC 9421 Ed25519 HTTP
Message Signatures over method, relay target URI, context, RFC 9530 content
digest and every forwarded end-to-end header. The proof has a nonce equal to
the invocation id and a validity of at most 60 seconds, bounded by grant expiry.
Unsigned forwarded headers, ambiguous framing and unsafe path encodings are
refused. The proxy atomically consumes the nonce across replicas and retains
its tombstone through grant expiry. A duplicate never replays a provider effect.

This explicit relay terminates its own HTTPS connection and creates a separate
verified HTTPS connection to the provider. It neither tunnels nor intercepts
the workload's provider TLS. CONNECT, TRACE and extension methods are refused.
The exact profile is [Connection HTTP Proxy](../reference/connection-http-proxy.md).
A transparent forward proxy would require a separately reviewed profile.

Optional future `secrets/exchange.EnsureProviderAccess` is a direct sealed
request/reply pair for registered trusted adapters. It has its own mode and
audience, uses the xkey bound to authenticated workload registration, and never
returns a refresh token. Its local validity bound does not change the provider's
token lifetime. It is not a prerequisite for proxy use.

The unpublished typed-operation fields are retired with reserved numbers and
names. Mandatory operation catalogs, operation refs and per-endpoint input/output
schemas are removed. The connection, grant, permission, consent and custody
lifecycle streams remain the control contracts.

## Alternatives Considered

### Require every provider call to be a typed operation

Superseded. It duplicates the provider's HTTP API in a platform operation
catalog, preventing ordinary proxy forwarding without an endpoint/schema
integration. Optional higher-level tools may use the proxy, but they do not
define the credential-use boundary.

### Give agent code the provider token

Rejected. The credential could be copied and used outside the connection's live
authorization. Token issuance to a registered trusted adapter is separate.

### Forward provider HTTP inside the secrets service

Rejected. Parsing untrusted requests and provider responses beside the only
OpenBao client crosses the isolation boundary of
[ADR#0023](./0023-secret-management-and-key-custody-direction.md).

### Treat a CONNECT tunnel as credential injection

Rejected. The tunnel cannot add an HTTP authentication header it cannot see.
The HTTP transport mechanism must be specified explicitly.

### Trust a grant identifier or inherit parent grants

Rejected. Identifiers are not bearer authority, and a child Session must pass
its own admission and permission checks.

### Infer fine-grained API permissions from generic HTTP forwarding

Rejected. Endpoint- or body-sensitive restrictions require a defined and
verified enforcement mechanism. Provider scopes continue to bound the
credential even where the proxy does not interpret an API body.

## Non-Goals

- Implementing a proxy or claiming runtime enforcement from schema checks.
- Selecting a deployment topology or transparent TLS interception.
- Replacing provider APIs with required per-endpoint protobuf schemas.
- OAuth consent and token custody, which
  [ADR#0094](./0094-oauth-authorization-protocol.md) and
  [ADR#0095](./0095-connection-credential-custody-and-refresh.md) own.
- Webhook ingress, non-OAuth onboarding and inbound MCP resource-server auth.
- Changing model-provider connections under
  [ADR#0032](./0032-model-route-and-credential-binding.md).

## Consequences

The platform authorizes and audits access to a connection while provider API
traffic stays HTTP. New endpoints do not require new protobuf operations merely
to pass through the proxy. Connector releases still pin provider trust,
authentication, custody and identity-probe metadata.

A compromised trusted proxy can expose bounded live access credentials. It
cannot read OpenBao, refresh credentials or app secrets, and its ability to
resolve a credential stays tied to live connection authority. Tenant isolation,
request limits and bounded caches remain necessary.

Proxy startup and live use refuse unavailable policy beyond the freshness
bound. Audit acceptance failure also stops dispatch. These are design
requirements that need runtime proof before rollout.

The companion drafts and protobufs use the same relay, authority and custody
boundary. [ADR#0063](0063-agent-connection-declarations.md)'s declared connection can acquire a proxy binding;
[ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)'s permits authorize that binding; [ADR#0074](0074-credential-platform-extraction-boundary.md)'s extracted custody supplies
the trusted caller; and [agent egress proxy proposal](https://github.com/TrogonStack/trogonai/pull/746)'s egress design must specify its relation to
credential-injecting proxy use. Existing model grants remain unchanged.

## References

- [Third-Party Connections](../architecture/third-party-connections.md)
- [Secret Management](../architecture/secret-management.md)
- [ADR#0023: Secret Management and Key Custody](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0049: Revocation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0063: Agent Connection Declarations](./0063-agent-connection-declarations.md)
- [ADR#0071: Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
- [ADR#0074: Credential Platform Extraction Boundary](./0074-credential-platform-extraction-boundary.md)
- [ADR#0077: API Key Authorization Model and Rate Limits](./0077-api-key-authorization-model-and-rate-limits.md)
- [ADR#0081: NATS Transport and Admission](./0081-nats-tls-by-default-and-operator-mode-admission.md)
- [ADR#0082: Bus Principal and Edge Proof](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
- [ADR#0083: Sealed Content Envelope](./0083-sealed-content-envelope.md)
- [ADR#0092: Third-Party Connection Model](./0092-third-party-connection-model.md)
- [ADR#0093: Connection Ownership and Delegation](./0093-connection-ownership-and-delegation.md)
- [ADR#0094: OAuth Authorization Protocol](./0094-oauth-authorization-protocol.md)
- [ADR#0095: Credential Custody and Refresh](./0095-connection-credential-custody-and-refresh.md)
- [policy enforcement posture proposal](https://github.com/TrogonStack/trogonai/pull/745): Policy enforcement posture (in review)
- [agent egress proxy proposal](https://github.com/TrogonStack/trogonai/pull/746): Agent egress proxy (in review)

## Operational defaults

The [security profile](../reference/connector-security-profile.md) fixes
tenant-isolated proxy identities/processes and body-free audit retention. The
[recovery contract](../reference/connector-relay-recovery.md) fixes durable
completion capacity/count/age and restore fencing. The
[provider admission profile](../reference/connector-linear-profile.md) selects a
versioned response-filter policy; a new provider cannot inherit an unreviewed
filter or assume arbitrary transformed credential echoes are detectable.

Deployment must prove tenant isolation, live revocation, nonce consumption,
bounded forwarding, custody authorization and audit outage behavior before
activation. These are implementation acceptance gates, not open design defaults.
