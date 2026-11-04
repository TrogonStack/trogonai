---
number: "0094"
slug: oauth-authorization-protocol
status: draft
date: 2026-11-03
---

# ADR#0094: The Platform Authorizes Third-Party Connections as an OAuth Client Through a Single-Use, Write-Ahead Attempt, and Fetches Untrusted Discovery Data Outside Custody

## Context

[ADR#0092](./0092-third-party-connection-model.md) defines a Connection to a
third-party provider and the aggregates around it. It does not say how a
principal's consent becomes one. That is an OAuth authorization code flow with
a browser in the middle: a redirect any host on the internet can request, a code
the provider honours once, and a provider account whose identity the platform
learns only after the code is spent.

Nothing in the repository acts as a third-party OAuth client today.
[ADR#0018](./0018-connectrpc-gateway-for-browser-product-surfaces.md) Decision 2
covers how an operator logs in to the console, not how the platform logs in to a
provider. The only MCP OAuth client the team has built so far is a
command-line tool outside this change. It implements part of the flow and stays
a protocol reference only: it does not read the `WWW-Authenticate` challenge,
build path-form metadata URLs, check the issuer, verify PKCE support, send
`resource` on exchange and refresh, or step up scope, and it stores tokens in
plaintext on disk. The inbound direction is also open:
`rsworkspace/crates/mcp/mcp-nats-server/src/main.rs:43-48` serves its HTTP
endpoint with no authorization, `rsworkspace/crates/mcp/mcp-nats-server/src/runtime.rs:44-56`
configures only allowed hosts, and the AAuth Access Server crate
(`rsworkspace/crates/aauth/trogon-aauth-as/src/lib.rs:1-5`) issues `aa-auth+jwt`
for organization resources, which is not an OAuth 2.1 authorization server.

The flow has failure modes the design must close, not merely handle:

- a redirect with a stolen or injected code, or a callback replayed;
- login CSRF, where one principal completes an authorization that another
  principal started;
- a crash or timeout after the code was sent, which leaves the provider holding
  a spent code and the platform holding nothing;
- two Connections claiming one provider account;
- a reconnect that silently swaps the account behind a Connection;
- a user-supplied MCP URL that makes the platform fetch internal addresses or
  re-point its own token endpoint; and
- a provider that grants less than was requested.

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 3
already argues that a standing OpenBao credential in a process that talks to
arbitrary remote servers turns any parsing or request-forgery bug into a pivot,
and its Consequences call the secrets service "the platform's highest-value
compromise target". Fetching a metadata document from a URL a tenant typed is
exactly that exposure, so where untrusted fetches run is part of this decision.

The remote MCP profile is pinned to the
[MCP authorization specification, revision 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization).
Storing the token set and refreshing it belong to
[ADR#0095](./0095-connection-credential-custody-and-refresh.md) and are not
restated here.

## Decision

### 1. One AuthorizationAttempt aggregate owns each consent, and it never holds material

An `AuthorizationAttempt` is an event-sourced aggregate in the connections
service ([ADR#0045](./0045-event-sourced-service-module-layout.md)), one stream
per attempt. It targets a ConnectionId, either a freshly reserved Pending
connection or an existing one being reconnected.

The attempt records an immutable custody `resource_parent`, equal to its target
Connection's parent. Captured authority, claims and staged material retain that
same project. Its public resource name is flat under the project, with the
Connection kept as an association. The [resource-parent reference](../reference/connector-resource-parents.md)
defines routing and payload equality checks.

| State | Entered by | Left by |
| --- | --- | --- |
| AwaitingConsent | BeginAuthorization | ClaimOAuthCallback, FailAuthorization, CancelAuthorization, ExpireAuthorization |
| CallbackClaimed | ClaimOAuthCallback | StartCodeExchange, FailAuthorization, CancelAuthorization, ExpireAuthorization |
| Exchanging | StartCodeExchange | CompleteAuthorization, FailAuthorization, RecordExchangeUnknown |
| OutcomeUnknown | RecordExchangeUnknown | reconciliation to Completed or Failed (Decision 4) |
| Completed, Failed, Expired, Cancelled | their commands | terminal |

The attempt holds the target ConnectionId, owner, the ProviderApp and
ConnectorRelease pins, the requested authority, the requested scope and
`resource`, the exact redirect URI used, the expected issuer, a capsule
reference (Decision 3), the state nonce id and signing key version, a keyed
digest of the browser binding and an opaque `SessionInitiatorBinding` (Decision 7), a
10 minute expiry, and for a reconnect the expected `authorization_generation`. It never
holds the authorization code, the PKCE verifier, the signed state, the
authorize URL, a token response, or provider error text, which is
provider-controlled and may echo any of them. A provider `error` is stored as a
normalized reason code. `authorization_attempts` is a sensitive stream under
[ADR#0092](./0092-third-party-connection-model.md) Decision 8, including the
precondition stated there on Decide replies.

`CancelAuthorization` and `ExpireAuthorization` are refused once the attempt is
Exchanging. A sent exchange cannot be un-sent, so an attempt that has crossed
that line resolves only to Completed, Failed, or OutcomeUnknown.

### 2. State is signed, single-use, and carries no material

The `state` parameter is an opaque, versioned token holding the attempt id, a
nonce id, an audience, an expiry, and the signing key version. The audience binds
it to one environment and one callback host, so state minted for staging does
not verify in production. It holds no tenant data a person could read, no
redirect URL, and no secret.

The secrets service signs it with a per-environment platform signing key that it
holds, separate from every tenant `KeyRef`. Authorization state is platform
control-plane state, which [ADR#0033](./0033-key-custody-product-model.md)
Decision 6 keeps out of KeyManagement purposes, and the private key never leaves
the secrets service
([ADR#0074](./0074-credential-platform-extraction-boundary.md)). The signature
proves authenticity only. Single use comes from the attempt stream:
ClaimOAuthCallback is a write conditioned on the attempt still being
AwaitingConsent, so exactly one claim wins and a replay meets CallbackClaimed
and gets the same neutral response as an unknown state. After a signing key
rotates, the old version verifies for the 10 minute state lifetime and no
longer, unless a compromise revokes it at once, in which case pending attempts
fail and users start again.

State is a defence against forged and replayed callbacks, not against an
unauthenticated user. It never authenticates a platform principal, which is why
Decision 7 requires a session.

### 3. The PKCE verifier lives in a custody transaction capsule

The platform sends PKCE with method `S256` on every authorization code flow.
At BeginAuthorization the connections service calls `OpenTransactionCapsule` on
the `secrets/exchange` interface. Custody generates the verifier, stores it in a capsule that expires
with the attempt, and returns only the challenge. At exchange the broker, inside
custody, calls `ConsumeTransactionCapsule` and uses the verifier in the token
request. The capsule is destroyed on consumption whether or not the exchange
succeeds, and there is no readback method.

The attempt records the capsule reference and nothing else about it. A capsule
orphaned by an attempt that never reached exchange expires on its own, and the
count of live capsules is an operational metric.

[ADR#0023](0023-secret-management-and-key-custody-direction.md) prefers that short-lived protocol state stay local to the process that
needs it. A verifier cannot: the callback may land on a different replica than
the one that built the redirect, a restart between them must not lose it, and it
must not sit in an event stream or any place a consumer can read. This needs a
narrow amendment to [ADR#0023](0023-secret-management-and-key-custody-direction.md) (Consequences).

A provider that cannot do PKCE is refused. The only exception is a reviewed
confidential-client declaration in its ConnectorRelease. The platform never
downgrades silently, and a generic MCP server without S256 support is refused
without exception (Decision 9).

### 4. The code exchange is written ahead, and a lost response is OutcomeUnknown

StartCodeExchange commits before any byte leaves for the provider. The event
records the attempt, the token endpoint origin taken from the pinned snapshot or
release, the redirect URI, and the `resource`. It does not record the code. Once
StartCodeExchange has committed, the connections service sends `code` and `state`
to the broker in a sealed `ExchangeAuthorizationCode` request, and the code
exists only in memory for the call.

If the provider response is lost, times out, or cannot be attributed, the
broker or a reaper over attempts stuck in Exchanging records
RecordExchangeUnknown. Reconciliation then runs in this order:

1. Ask custody whether a staged token bundle exists under this attempt id. If it
   does, the exchange succeeded: continue to the identity probe (Decision 5).
2. Otherwise the code is presumed spent. The platform never reuses it, because a
   second redemption of a code lets a provider revoke the tokens it issued from
   the first. The attempt fails with a typed reason and the principal starts a
   new attempt.
3. A grant the provider may have recorded without the platform holding its
   tokens is a residual the principal can remove in the provider's own settings.
   It is recorded on the attempt as an unknown cleanup obligation, not hidden.

If the code or verifier is lost before the exchange is sent, the attempt fails
the same way and a new attempt starts. Cancellation cannot make a sent exchange
disappear, which is why Decision 1 refuses it.

The pinned `OAuthProfile` also owns wire differences at the authorization and
token endpoints. `OAuthRequestParameters` declares bounded public constants for
authorization, code exchange and refresh. Parameter names are unique within a
stage. Protocol and credential fields, including state, client identity,
redirect URI, PKCE, resource, scope and grant material, remain dispatch-owned
and cannot be overridden. Constants contain no credentials or personal hints;
values are encoded once as data, not interpolated into a URL or form.

An absent `OAuthResponseMapping` selects the RFC 6749 top-level field names and
types. A declared mapping selects exact JSON pointers and lifetime/scope
encodings; custody never searches alternative paths or coerces malformed
values. Access material is a nonempty string and token type must be Bearer.
A reviewed declared-endpoint release may explicitly document Bearer when the
provider omits the type; a present conflicting or malformed type still fails.
Generic discovered MCP cannot select that fallback. Missing refresh material
follows the release's initial/refresh omission rules, missing scope follows the
scope rule, and missing access lifetime follows the explicit expiry policy of
[ADR#0095](./0095-connection-credential-custody-and-refresh.md).

Duplicate JSON member names, conflicting selectors, nulls, wrong types,
overflowing or fractional lifetimes and missing required fields fail before
staging. Unknown response members are discarded, and token responses are capped
at 16 KiB. A malformed success during refresh may follow provider-side rotation:
it is an unknown outcome, never permission to replay the previous refresh token.
These OAuth response rules do not impose a schema on ordinary provider API
responses, which use the [HTTP proxy profile](../reference/connection-http-proxy.md).

### 5. The token bundle is staged, the account is probed, and the claim is held before activation

The sequence after the callback is fixed, and each step is durable before the
next begins:

| Step | Owner | Durable record |
| --- | --- | --- |
| Claim the callback | connections service | CallbackClaimed |
| Start the exchange | connections service | Exchanging (write-ahead) |
| Exchange the code, stage the bundle as not yet usable | broker in custody | staged bundle keyed by attempt id |
| Run the release's identity probe with the staged access token | trusted executor | ProviderAccount observed |
| Capture the granted authority and compare it (Decision 12) | connections service | granted authority on the attempt |
| HoldProviderAccount | connections service | claim Held |
| ActivateConnection | connections service | configuration, bindings, claim Bound, `authorization_generation` |
| Promote the staged bundle to the connection's token set | custody | compare-and-set write |
| CompleteAuthorization | connections service | Completed |

The identity probe runs in the trusted executor, never in the broker. The
executor resolves the staged access token through `ResolveForTrustedUse` under
an attempt-scoped authorization (the attempt id plus a staged bundle reference),
as [ADR#0095](./0095-connection-credential-custody-and-refresh.md) and
[ADR#0096](./0096-connection-use-by-agents.md) define it.

The `ProviderAccountClaim` is keyed by a keyed fingerprint (computed in custody
so stream names do not leak account ids) over tenant, custody project, owner,
provider, issuer, app boundary, subject kind, account, subject, and
installation. It is created with a no-stream precondition, so two concurrent
attempts for the same account cannot both succeed. Its states are Held(connection),
Bound, and Released. A claim is never stolen on a timeout: the holding
connection must be terminally aborted or revoked before the claim moves.
`account_claims` is a sensitive stream under [ADR#0092](0092-third-party-connection-model.md) Decision 8.

The claim cannot be taken at BeginAuthorization, because the account is unknown
until the probe. It is therefore taken after the exchange and before
activation. An active duplicate is not overwritten and its consent is not
touched. The losing attempt fails with a typed `ACCOUNT_ALREADY_CONNECTED`
reason, its staged bundle is destroyed locally, and the console offers a
targeted reconnect of the existing connection. The platform does not call the
provider's revoke on the losing token, because some providers treat revocation
of one token as revocation of the whole grant the existing connection also uses.
The discarded token is recorded as a residual until it expires. If a crash
splits activation from promoting the staged bundle and CompleteAuthorization, a
reconciler finishes both from the durable events. The staging rules, meaning the
attempt-keyed path, the promotion at activation, and the destruction of a bundle
whose attempt is lost, aborted, or expired, belong to
[ADR#0095](./0095-connection-credential-custody-and-refresh.md).

### 6. Reconnect keeps the ConnectionId, requires the same account, and re-consents

A reconnect is a new attempt on the existing ConnectionId. It carries the
expected `authorization_generation`, so a stale tab cannot reconnect over a newer
consent. It always asks the provider for fresh consent, captures the scopes the
provider actually granted, and runs the same probe and the same claim logic. Its
bundle is staged without touching the live token set until activation.

The fingerprint from the probe must equal the one the Bound claim holds, and
subject continuity must hold where the provider exposes a stable subject.
Otherwise the attempt fails with `ACCOUNT_MISMATCH`, the new bundle is
destroyed, and the connection is unchanged. This is the continuity rule
[ADR#0032](./0032-model-route-and-credential-binding.md) Decision 8 sets for a
model provider connection, applied here. On success the
`authorization_generation` increments and bindings and permits attached to the
ConnectionId carry over. Credentials and grants of an earlier
`authorization_generation` stay revoked and are never revived.

Revoke and reconnect stay different acts. A principal revoking a connection is
intent: it is terminal and a new ConnectionId is needed. A provider uninstalling
the app, or refresh returning `invalid_grant`, is fact: the connection becomes
ReconnectRequired and keeps its id.

A step-up is a reconnect that clears the `SCOPE_STEP_UP` health reason
(Decision 11). For the pinned MCP authorization profile, its requested scope is
the union of the previous attempt's requested scopes and the challenged scopes,
with every value checked against current release/app/policy ceilings. The prior
attempt is reached through the authorization record; missing history fails
closed. This does not confuse requested scope with provider-granted authority. The platform never opens a consent screen
unprompted. The console shows the reconnect and a principal starts it.

### 7. The callback is a gateway route that requires the initiating principal's session

**Host.** The console gateway of
[ADR#0018](./0018-connectrpc-gateway-for-browser-product-surfaces.md) hosts the
callback as a plain HTTPS `GET` route outside the ConnectRPC allowlist of
Decision 4 there. The provider redirect is a top-level navigation with no custom
headers, so it cannot be a ConnectRPC method, and the gateway already owns the
browser session. The route is mechanical in the sense of that ADR's Decision 3:
it carries `code`, `state`, `error`, and `iss` plus verified authentication
context to trusted dispatch at the connections service. It never supplies a
principal field in an aggregate command and holds no rule. The secrets service is not the
host, because it would become an internet-facing process with the OpenBao
client.

**Transport.** The callback edge passes `code` and `state` to the connections
service over core NATS request/reply, with the request and the reply sealed to
the receiver's key, and the connections service sends them onward to the broker
as Decision 4 describes. That is the rule
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
Decision 5 sets for external credentials, using the sealing of [ADR#0083](0083-sealed-content-envelope.md). The
code is never published to JetStream and never reaches a log.

**URIs.** Each environment registers exactly one HTTPS callback URI per platform
ProviderApp on that environment's console host. There is no wildcard, no path
parameter, and no caller-supplied `redirect_uri` or post-login destination. The
attempt records the URI it sent and the exchange sends the same string. After
the claim the route answers `303` to a console route computed on the server from
the attempt's connection and outcome, never from a request parameter. A local
development environment uses its own provider app and loopback URI, and never a
production app.

**Binding.** The route requires the platform session cookie, which must be
`SameSite=Lax` so a provider's top-level redirect carries it. A request without
a session is rejected before any NATS request or custody call. The same verified
principal must begin and claim the attempt, but neither command carries that
principal. At begin, trusted dispatch computes `SessionInitiatorBinding` with a
versioned platform callback-binding key distinct from the state-signing key.
The HMAC-SHA256 input is the UTF-8 sequence
`connection-oauth-initiator-v1`, verified tenant account id, attempt id and
verified session principal id. Each component is prefixed by its byte length
as a four-byte unsigned big-endian integer. The record contains only the key
version and the 32-byte digest.

At claim, dispatch uses the attempt's pinned key version and the current verified
session identity to recompute the binding, never an identity or digest supplied
in request input. The decider checks the digest in constant time against the
stored binding. Keys remain available for open attempts; unknown or revoked
versions fail closed. `CallbackBinding.initiating_principal` and
`ClaimOAuthCallback.session_principal` are retired field names and numbers, not
alternate authentication inputs. Envelope actor provenance remains in the
runtime envelope.

The browser binding is also required: BeginAuthorization sets a short-lived
`__Host-` prefixed `HttpOnly` cookie holding a random value, and the attempt
stores its keyed digest. Claim requires both the initiator binding and browser
cookie binding to match, so an authenticated different principal or a link
forwarded to another browser fails. [ADR#0018](0018-connectrpc-gateway-for-browser-product-surfaces.md) Decision 2 puts Origin checks on state-changing ConnectRPC
requests. A provider redirect has no meaningful Origin, so the callback is a
documented exception whose defences are the signed single-use state, the
initiator match, and the browser binding.

**Response and logging.** The route decodes the form response once and compares
`iss` to the recorded issuer by exact string equality, as in RFC9207. It applies
no case folding, slash/port or percent-encoding normalization. Issuer validation
also precedes acting on or displaying provider error fields; mismatch refuses
the callback.
It refuses a response without `iss` when the issuer's metadata advertises
`authorization_response_iss_parameter_supported`. Every reply is `no-store` with
`Referrer-Policy: no-referrer`, and the `303` navigates to a clean console URL.
It does not claim to erase browser or intermediary history already recorded. Access logs, traces, and error reports redact the query string
of this path, and a CI check fails if a log field can carry it.

**Limits.** Only the session-holding edge calls custody, so an anonymous flood
costs a cookie check and nothing more. The edge caps query size, accepts only
the named parameters, rate-limits per session and per source address, and
counts claim failures per attempt. The selected limits are in the connector security profile.

**Region.** [ADR#0033](./0033-key-custody-product-model.md) Decision 4 has no
regional boundary today: there is one deployment, one console host, and so one
callback host. Providers bind exact redirect URIs, so a callback host is a
long-lived registration and moving it is a migration at every provider. If
regional deployments arrive, an in-flight attempt keeps the URI it recorded and
is never moved between regions, and region becomes a routing input on the
attempt in the way that ADR describes for key routing. This ADR does not decide
which region would own a URI, and records the constraint.

### 8. Untrusted fetches run in a sandboxed fetcher, and only token calls run in custody

A **discovery fetcher** performs every fetch whose target a tenant or a remote
server chose and parses the untrusted documents: protected resource metadata,
authorization server metadata, and the validation of a registration endpoint. It
holds no secret, no OpenBao client, and no NATS permission beyond receiving a
fetch request and returning a typed result. It runs as its own workload, so a
parser or request-forgery bug in it reaches nothing worth stealing.

The **broker** inside custody performs only calls that carry material: the code
exchange, refresh, token revocation, the dynamic registration `POST` (whose
response carries a client secret and a registration access token), and RFC 7592
delete. It sends them through a platform egress path pinned to the origins in
the discovery snapshot of Decision 10. The pin is enforced at the network layer
as well as in code, so a compromised broker cannot reach another host.

This qualifies the placement of the broker as a module inside a minimal
secrets service, which stays the shape. The module is bounded: it fetches
nothing a tenant or a remote server chose, and it parses only the narrow, typed,
size-capped responses of the token, revocation, and registration endpoints that
the fetcher validated first. The fetcher, not the broker, carries the exposure
[ADR#0023](0023-secret-management-and-key-custody-direction.md) Decision 3 warned about, and it sits where a compromise is cheap. The
identity probe calls a provider API with the staged access token and runs in the
trusted executor (Decision 5), so the broker never parses a provider API
response.

The fetcher and the egress path follow the endpoint rules of [ADR#0032](0032-model-route-and-credential-binding.md)
Decision 8, extended for untrusted input:

- HTTPS only, with hostname verification and no URL userinfo.
- Public addresses only. Loopback, link-local, private, carrier-grade NAT,
  unique-local, multicast, and cloud metadata addresses are refused, including
  IPv4-mapped forms.
- Resolve each hop, connect to the validated address, and verify the certificate
  against the original host. Pin per hop, so DNS rebinding between check and use
  fails.
- No redirect on a request that carries material. A discovery `GET` may follow a
  same-origin redirect up to a small hop limit, revalidated per hop, and never a
  cross-origin one.
- A size cap, a content-type check, a JSON depth cap, a connect and total
  deadline, and a limit on concurrent fetches per tenant.
- Revalidation of the origin before any credential-bearing request after a DNS
  or configuration change.

### 9. The MCP profile discovers in specification order and refuses what it cannot verify

For a generic remote MCP server the fetcher runs this order, and a step that
fails does not fall through to a guess:

1. Make an unauthenticated request to the MCP URL and parse `WWW-Authenticate`
   for `resource_metadata` and `scope`.
2. Without `resource_metadata`, request RFC 9728 metadata at
   `/.well-known/oauth-protected-resource/<path>`, then at the root form. None
   found means failure, or a preregistered app if the principal supplied one. The
   platform never assumes the MCP origin is the authorization server.
3. Require the metadata `resource` to match the MCP URL, or to be a path prefix
   of it on the same origin for the root form. A mismatch is refused.
4. For each listed authorization server in order, fetch its metadata at the
   RFC 8414 path-insertion URL, then the OIDC path-insertion URL, then the OIDC
   path-appending URL (issuers without a path use the two plain forms).
5. Require `issuer` to equal the issuer URL used to build the request.
6. Refuse an authorization server whose `code_challenge_methods_supported` is
   absent or lacks `S256`. The specification requires this check, and the OIDC
   form is held to it as well.
7. Use the first authorization server that passes, record the choice with the
   protected resource metadata on the Connection, and refuse the connection if
   none passes.

### 10. Registration follows specification priority, and discovery is kept as a snapshot

Registration ends as a ProviderApp, in this order:

1. A preregistered client the principal supplied: a `CUSTOMER_SUPPLIED`
   `OAuthClientApp`, with its secret written to custody first.
2. A Client ID Metadata Document, when the metadata sets
   `client_id_metadata_document_supported`. The platform hosts one document per
   environment, not one per tenant, at a stable HTTPS URL with a path, served by
   the same edge as the callback and generated from configuration. Its
   `client_id` equals its URL exactly, its `redirect_uris` are the
   environment's callback URI, and it is a public client by default, or
   `private_key_jwt` through `SignJwt` when a server requires it. It holds no
   tenant data and creates no per-tenant registration state.
3. RFC 7591 dynamic registration, as a `DYNAMICALLY_REGISTERED` `DynamicClient`.
   The app is reserved before the HTTP call, and the client secret and
   registration access token go to custody before anything is recorded.
   `registration_client_uri` is capability-bearing and protected. An ambiguous
   response is not retried without bound, and a possible orphan registration
   becomes a recorded residual. DCR is a deprecated compatibility fallback in
   the pinned2026 authorization profile, never preferred over CIMD. Send
   application_type=web for this server-hosted callback; native registration
   needs a separate profile.
4. Otherwise the principal enters client details, which is the preregistered
   path again.

An app is identified by owner, issuer, registration method, and `client_id`. A
change to the issuer or the `client_id` is a new app.

The **discovery snapshot** is the validated metadata bytes, their digest, the
fetch time, the specification revision, and the chosen issuer. Authorization
server metadata is recorded on the ProviderApp event, and `OAuthClientApp`, in its
Client ID Metadata Document form too, carries `issuer` and a reference to that
snapshot. Protected resource metadata is recorded on the Connection, because it
is per resource. The exchange, refresh, revoke, and delete calls read their
endpoints from the snapshot and never from a fresh fetch, so a server cannot
re-point the token endpoint after consent. A new discovery makes a new snapshot
version. A change to the set of endpoint origins needs a new acknowledgment from
the principal, and a digest change with the same origins is recorded without one.

For a generic MCP connection the egress hosts come from the snapshot, not a
reviewed release. This is a deviation from the posture of
[ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md),
which binds what an Agent may reach as policy on a subject: the hosts are carried
by the connection, learned from the server being connected, and this ADR adds no
egress arm to that policy. The consent screen lists the MCP origin and the
issuer, authorization, token, and registration origins, and the principal
acknowledges them at connect time. Curated servers ship their own release with
fixed hosts and need no acknowledgment.

### 11. The `resource` parameter, scope selection, and step-up follow the specification

The canonical resource URI is the MCP server URI with a lowercase scheme and
host, no fragment and no query in this supported profile. A semantically
significant trailing path slash is preserved; unsupported URI shapes are refused,
never silently normalized to another resource. One function builds it, and
the Connection stores it with the issuer. The platform sends it as the RFC 8707
`resource` on the authorize, code exchange, and refresh requests, even when the
authorization server ignores it. A different resource URI is a different
Connection, even on the same server and issuer.

The requested scope is the challenged `scope` when present, else the metadata
`scopes_supported`, else omitted. The platform treats a challenged scope set as
authoritative and assumes no relation to `scopes_supported`.

For refresh-capable clients, generated CIMD/DCR metadata includes the appropriate
refresh_token grant type when supported. Request offline_access only when the
authorization-server metadata advertises it and the profile needs it; resource
metadata alone does not authorize that request. Missing refresh support is
explicitly nonrefreshable, not an invented refresh credential. The
[MCP authorization reference](../reference/connector-mcp-authorization.md) pins
these2026 authorization rules independently of transport/session versions.

A `403` with `error="insufficient_scope"` leaves the connection Active with the
health reason `SCOPE_STEP_UP` and the challenged scopes recorded. Only the tool
calls that need those scopes fail, with `PROVIDER_INSUFFICIENT_SCOPE`; the rest
keep working. The trusted MCP executor reports the observation to the connections
service through `ReportConnectionHealth`, and that service and custody are the
only writers of Connection health facts. Step-up re-consent goes through the
reconnect flow of Decision 6 on the same ConnectionId, and the reconnect must
return the same account. A cap on step-up attempts per connection and operation
stops a server from looping the user through consent. When it is reached, the
operation fails as permanently unauthorized until a principal reconnects on
purpose.

A `401` makes the executor ask custody for one forced refresh, through
`ResolveForTrustedUse` with a force-refresh flag
([ADR#0095](./0095-connection-credential-custody-and-refresh.md)). If the refresh
fails with `invalid_grant`, custody records ReconnectRequired. Refresh never
authorizes automatically repeating a potentially effective tools/call. Only
protocol/control requests with established safe/idempotent semantics may use a
bounded retry policy. A second generic401 is an authentication observation,
not proof that the grant is dead; only authoritative custody or a reviewed
trusted probe may establish that health transition.

There is no token passthrough. The platform sends a token only to the resource it
was issued for, using `Authorization: Bearer` on every request and never in a
query string, and never forwards a token received from a caller, an agent, or
another server. Ordinary provider requests use the authorized HTTP proxy of
[ADR#0096](./0096-connection-use-by-agents.md), with provider credentials held
only in trusted proxy memory. MCP still has a trusted protocol executor, which
holds its token in memory and closes the MCP session on revoke. The proxy
boundary does not require a typed operation or response mapping for each
provider API endpoint; the OAuth token endpoint parser and the account identity
probe remain provider-specific trusted code.

### 12. Granted scope is compared with requested scope, and a reduction is a health reason

At exchange, the granted scope from the token response is compared with the
requested scope. An absent response `scope` means the grant equals the request,
as in RFC 6749. Comparison uses the release's declared scope normalization,
never substring matching. A profile whose omission proves nothing instead
requires the release's scope probe.

Refresh compares with the current committed granted authority, not the scopes
of the original consent. The refresh request never asks for a wider set. With
no narrower refresh scope requested, an omitted response scope retains that
current set; if the request deliberately narrows it, the documented omission
rule applies to that narrower request. A prior reduction is never undone merely
because a later refresh omitted `scope`.

If the granted set lacks a scope the release marks as required, the attempt fails
and nothing activates. If it lacks only optional scopes, the connection
stays Active with the authority the provider actually granted and carries the
health reason `SCOPES_REDUCED`. The reason is not a Degraded trigger. A refresh
that reports a narrower scope lowers the granted authority: custody reports the
reduced authority as a health fact appended to the Connection stream, which is an
allowed exception to the rule that refresh never appends to it, and the same
reason is set. A refresh that reports wider actual authority is quarantined as
`AUTHORITY_WIDENED`: custody marks the family Unusable, refuses resolve, records
cleanup debt and requires reconnect. The candidate is never promoted under
narrower metadata, and a potentially consumed refresh credential is not retried. Admission and the
proxy and trusted MCP caller read the granted authority from the Connection
projection. Provider scopes bound the credential; a generic HTTP proxy does not
infer API-specific scope requirements from an arbitrary request body. The refresh
transaction itself belongs to [ADR#0095](0095-connection-credential-custody-and-refresh.md).

### 13. Disconnect and offboarding revoke at the provider where the provider allows it

RevokeConnection commits first, so denial is immediate and caches evict within
the [ADR#0049](./0049-revocation-latency-target.md) bound. A cleanup worker then
calls the provider from custody, refresh token first because RFC 7009 lets a
provider invalidate dependent access tokens when a refresh token is revoked. It
records each result as an `EffectOutcome`:

| Outcome | Meaning | Handling |
| --- | --- | --- |
| CONFIRMED | The `revocation_endpoint` answered success | Done |
| UNSUPPORTED | No `revocation_endpoint` advertised or declared | Record a residual: tokens live until their expiry, and the principal can remove the app in the provider's settings. Never faked |
| REJECTED | The provider refused the request | Record a residual, no automatic retry for a permanent error |
| UNAVAILABLE | The provider failed or was unreachable | Retry with backoff for a bounded window, then record a residual |
| UNKNOWN | The request was sent and no response arrived | Treated as UNAVAILABLE, since revocation is idempotent |

Dynamically registered clients are deleted with RFC 7592 when no connection uses
the app any longer, only for registrations with an exact ownership receipt, and
a missing `registration_client_uri` records a residual registration. The same
path serves project offboarding of
[ADR#0093](./0093-connection-ownership-and-delegation.md). Material is destroyed
only after the revoke outcome is terminal or the window ends, and
[ADR#0095](./0095-connection-credential-custody-and-refresh.md) owns destruction
and retention. Residuals are durable, queryable, and aged in a metric.

## Alternatives Considered

**Host the callback in the secrets service.** Rejected. It would be an
internet-facing process holding the sole OpenBao client, the exposure [ADR#0023](0023-secret-management-and-key-custody-direction.md)
Decision 3 exists to avoid.

**Host the callback in a dedicated minimal service.** The leading alternative.
It isolates the route, but the route must bind to the initiating principal's
console session, so the service would need either the gateway's session store or
its own login, and it would add a surface to operate. Kept as the fallback if the
gateway cannot host a non-ConnectRPC route without breaking its default-closed
exposure rule.

**Run discovery inside the broker.** Rejected: every tenant-typed URL would be
fetched, and every untrusted document parsed, inside the compromise target.

**Run the registration `POST` in the fetcher.** Rejected: the response carries a
client secret and a registration access token, which a process holding no
secrets must not see. The fetcher validates the endpoint and custody posts.

**Store the verifier on the attempt, sealed.** Rejected. The attempt stream is
readable by the connections service, and a verifier there is readable outside
custody.

**Take the account claim at BeginAuthorization.** Impossible: the account is
unknown until the probe. A reservation keyed on the ConnectionId would not stop
two Connections from binding one account.

**Make a state-only callback with no session.** Rejected. State authenticates
nothing about the user, and it would leave login CSRF open.

**One client registration per tenant for every MCP server.** Rejected in favor
of the Client ID Metadata Document, which needs no per-tenant state to clean up
and is the specification's recommended default for a client with no prior
relationship to the server. Dynamic registration remains the fallback where it is
the only option.

**Retry a lost exchange with the same code.** Rejected. A spent code may revoke
the tokens its first redemption issued, and the platform cannot tell which case
it is in.

## Non-Goals

- **Inbound MCP.** Trogon acting as an OAuth-protected MCP server for external
  clients needs its own ADR. The HTTP server in `mcp-nats-server` serves with no
  authorization today, and the AAuth crates are not an OAuth 2.1 authorization
  server. Nothing here changes either.
- **Token storage, refresh, retention, restore, and key loss.** These are
  [ADR#0095](./0095-connection-credential-custody-and-refresh.md).
- **Who may use a connection and how an agent calls through it.**
  [ADR#0096](./0096-connection-use-by-agents.md).
- **Ownership, delegation, and offboarding policy.**
  [ADR#0093](./0093-connection-ownership-and-delegation.md).
- **Non-OAuth credentials**: API keys, basic credentials, service accounts, and
  client credentials flows.
- **Webhook registration and delivery** for providers.

## Consequences

- Amendments are required. [ADR#0023](0023-secret-management-and-key-custody-direction.md) gains the transaction capsule and states that
  the broker fetches no tenant-chosen URL. [ADR#0074](0074-credential-platform-extraction-boundary.md) names the connections service
  as another consumer of the secrets service and the discovery fetcher as a
  separate workload. [ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md) notes that the hosts of a generic MCP connection
  come from its snapshot. [ADR#0083](0083-sealed-content-envelope.md) Decision 1 declares `authorization_attempts`
  and `account_claims` sensitive streams, as [ADR#0092](0092-third-party-connection-model.md) Decision 8 names them.
  [ADR#0018](0018-connectrpc-gateway-for-browser-product-surfaces.md) adds a plain callback route outside the ConnectRPC allowlist and pins the session cookie to `SameSite=Lax`. [ADR#0033](0033-key-custody-product-model.md)
  records that callback and redirect URIs have no regional owner.
- The platform operates a new workload that holds no secrets, and an egress path
  enforced at the network layer. The egress path also serves the exchange,
  refresh, and revoke calls of every connector, so its cost is shared.
- A single Client ID Metadata Document means one `client_id` for every tenant at
  an authorization server. If a server revokes it, every tenant of that server
  must reconnect, and provider rate limits are shared.
- Every consent has a 10 minute deadline and a console session requirement.
  A user who lets a tab sit, or who opens the link in another browser, starts over.
- A lost exchange response costs the user one new consent. That is the price of
  never reusing a spent code, and it is accepted.
- Providers have no uniform revoke. `UNSUPPORTED` and `REJECTED` outcomes leave
  tokens alive until expiry, and the platform reports that rather than hiding it.
- Generic MCP reaches hosts the platform has not reviewed. The acknowledgment, the
  pinned snapshot, and the egress pin are the compensating controls, and they
  require the principal to read the host list.
- The existing command-line MCP client keeps its current behavior until it
  migrates to this profile, and is not a basis for the implementation.

## Operational defaults

The [security profile](../reference/connector-security-profile.md) fixes console
callback/CIMD origin ownership, callback/step-up budgets, ten-minute capsule/state
lifetime, sandboxed discovery and fetch limits. V1 requires S256 PKCE and has no
automatically granted confidential-client exception. A release declaring the
schema's exemption arm remains activation-disabled until a separately reviewed
provider exception exists. A future regional deployment must retain the exact
registered callback origin and route verified attempts internally; it cannot
accept an arbitrary redirect origin.

The browser-facing orchestration is the
[control API](../reference/connector-control-api.md); the internal domain command
is not a browser onboarding request. Raw probe identity reaches only the
[attempt-authorized custody fingerprint operation](../reference/connector-account-identity.md).

## References

- [ADR#0018: ConnectRPC Gateway for Browser Product Surfaces](./0018-connectrpc-gateway-for-browser-product-surfaces.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0033: Key Custody Product Model](./0033-key-custody-product-model.md)
- [ADR#0045: Aggregate-Oriented Module Layout for Event-Sourced Services](./0045-event-sourced-service-module-layout.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0071: What an Agent May Reach, How Far It May Go, and Whether It May Run at All Are Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
- [ADR#0074: Credential Platform Extraction Boundary](./0074-credential-platform-extraction-boundary.md)
- [ADR#0082: Inside the Bus the Principal Is Derived from the Subject, and Proof of Possession Is Enforced Only at the External Edge](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
- [ADR#0083: Credential Material, Claim-Check Blobs, and Sensitive Tenant Streams Under a Customer Managed Key Are Sealed in an Owned AEAD Envelope, and Nothing Else Is](./0083-sealed-content-envelope.md)
- [ADR#0092: Third-Party Connection Model](./0092-third-party-connection-model.md)
- [ADR#0093: Connection Ownership and Delegation](./0093-connection-ownership-and-delegation.md)
- [ADR#0095: Connection Credential Custody and Refresh](./0095-connection-credential-custody-and-refresh.md)
- [ADR#0096: Connection Use by Agents](./0096-connection-use-by-agents.md)
- [MCP authorization specification, revision 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)
- [RFC 6749: The OAuth 2.0 Authorization Framework](https://datatracker.ietf.org/doc/html/rfc6749)
- [RFC 7009: OAuth 2.0 Token Revocation](https://datatracker.ietf.org/doc/html/rfc7009)
- [RFC 7591: OAuth 2.0 Dynamic Client Registration Protocol](https://datatracker.ietf.org/doc/html/rfc7591)
- [RFC 7592: OAuth 2.0 Dynamic Client Registration Management Protocol](https://datatracker.ietf.org/doc/html/rfc7592)
- [RFC 7636: Proof Key for Code Exchange](https://datatracker.ietf.org/doc/html/rfc7636)
- [RFC 8414: OAuth 2.0 Authorization Server Metadata](https://datatracker.ietf.org/doc/html/rfc8414)
- [RFC 8707: Resource Indicators for OAuth 2.0](https://datatracker.ietf.org/doc/html/rfc8707)
- [RFC 9207: OAuth 2.0 Authorization Server Issuer Identification](https://datatracker.ietf.org/doc/html/rfc9207)
- [RFC 9700: Best Current Practice for OAuth 2.0 Security](https://datatracker.ietf.org/doc/html/rfc9700)
- [RFC 9728: OAuth 2.0 Protected Resource Metadata](https://datatracker.ietf.org/doc/html/rfc9728)
- [OAuth Client ID Metadata Document](https://datatracker.ietf.org/doc/html/draft-ietf-oauth-client-id-metadata-document-00)
