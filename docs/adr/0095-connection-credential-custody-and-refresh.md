---
number: "0095"
slug: connection-credential-custody-and-refresh
status: draft
date: 2026-11-03
---

# ADR#0095: Each Connection's OAuth Token Set Is One Sealed, CAS-Written OpenBao Secret, Refreshed Only Through a Write-Ahead Credential Transaction

## Context

An OAuth credential combines access authority with provider validity evidence,
a refresh token that many providers rotate on every use, and the attribution of
which refresh produced the pair. A provider can omit the access lifetime, so
expiry evidence cannot be represented by a mandatory guessed timestamp. [ADR#0092](./0092-third-party-connection-model.md) binds a
Connection to an opaque `SecretRef`,
[ADR#0094](./0094-oauth-authorization-protocol.md) obtains the first token set,
and [ADR#0096](./0096-connection-use-by-agents.md) spends it. None of them says
how the set is stored, refreshed without being lost, or protected when keys,
backups or providers misbehave.

The store cannot hold one safely today. `OpenBaoWriteRequest` carries a single
string and no `cas` option
(`rsworkspace/crates/platform/trogon-gateway/src/secret_store/openbao_secret_store.rs:480`),
`rotate` writes unconditionally
(`rsworkspace/crates/platform/trogon-gateway/src/secret_store/openbao_secret_store.rs:325`),
`CredentialKind` has no token-set kind
(`rsworkspace/crates/platform/trogon-gateway/src/credential/commands/domain/credential_kind.rs:4`),
and `SecretString` is an `Arc<str>` that is never zeroized
(`rsworkspace/crates/platform/trogon-std/src/secret_string.rs:5`).

A refresh is also a remote mutation. A provider that rotates refresh tokens
treats a replayed one as theft and may revoke the grant family, and no local
transaction spans the provider's response and the platform's write. A lease that
can expire while the request is in flight cannot be what prevents a second send.

The accepted decisions leave a gap around both.
[ADR#0047](./0047-event-sourced-credential-metadata.md) makes the credential
event stream the source of truth for metadata.
[ADR#0074](./0074-credential-platform-extraction-boundary.md) moves the adapter
and aggregate into the secrets service behind five policies, none of which lets
a runtime write a token set. [ADR#0083](./0083-sealed-content-envelope.md) keeps
OpenBao KV values outside a [customer managed
key](../glossary/customer-managed-key)'s veto, so destroying that key erases the
sealed records and leaves live refresh tokens readable. [Secret
Management](../architecture/secret-management.md) leaves open the
resolved-value wrapper, the resolve failure taxonomy and anomaly detection.

## Decision

For ordinary provider traffic, the trusted caller in this ADR is the authorized
connection HTTP proxy of [ADR#0096](./0096-connection-use-by-agents.md). References
to an executor also cover the trusted MCP executor and the staged identity-probe
worker where those protocols require one. They do not require an
`InvokeOperation` RPC for an ordinary HTTP request. See the [connection
architecture explanation](../architecture/third-party-connections.md) for the
HTTP and protobuf boundary.

### 1. A token set is one structured secret, written with compare-and-set before use

A new `OAUTH_TOKEN_SET` kind, with a purpose-limited broker-use policy, holds one
JSON object per connection: access and refresh tokens, token type, access-expiry
evidence, refresh expiry when known, granted scope, issuer, canonical resource,
`bundle_generation`, and the
`operation_id` and fence of the write that produced it. The tokens share one
value because they rotate as a pair, and two writes cannot be atomic. Existing
enum numbers are preserved. Non-secret facts (generation, expiries, scopes,
outcomes) are credential-aggregate events; no token enters an event, snapshot,
KV bucket, log, trace or error.

The credential aggregate is keyed by its opaque `SecretRef`, because custody
also holds app registration material and staged bundles, not only live
connection tokens. A staged bundle and the live token set are separate streams.
The reference is an identity, never an OpenBao path.

Credential metadata records a separate immutable `CredentialResourceParent`.
Connection and staged-attempt material is project-scoped; platform ProviderApp
material is platform-scoped. A subject reference must agree with that container,
and promotion keeps source and destination material in the same project. The [resource-parent
reference](../reference/connector-resource-parents.md) defines public names and
route checks; an opaque SecretRef does not authorize access to its parent.

`TokenBundle.access_expiry` carries an `AccessExpiry` evidence oneof:

| Evidence | Meaning |
| --- | --- |
| `reported_expires_at` | Derived from a valid reported lifetime and custody's response receipt time |
| `documented_expires_at` | Derived from the pinned release's documented fallback lifetime and receipt time |
| `unknown` | No provider expiry evidence; no timestamp is fabricated |
| `non_expiring` | The pinned release explicitly documents no scheduled access expiry |

A reported lifetime takes precedence over an omission fallback. An absent
omission policy means unknown expiry. Documented fallback lifetime and
non-expiring access require a reviewed declared-endpoint profile, never generic
discovered MCP. Null or malformed lifetime values are errors, not omissions;
zero denotes expired access. The old required `provider_expires_at` field is
reserved rather than repurposed. Local cache validity remains finite for every
evidence arm.

The store port gains a write that takes the expected version and returns the
written one. A key is created with `max_versions = 2` and `cas_required = true`,
first written with `options.cas = 0`, and every later write sends the version it
read, so OpenBao itself refuses an unconditional write. A mismatch means another
writer won; the loser re-reads and never retries blindly. The value is written
before it is used and before the stream is told, because the stream records what
custody holds.

Each refresh adds a version that may hold a live access token. `max_versions = 2`
keeps the current version and its predecessor, the least a recovery pass needs to
compare a candidate with the last committed bundle. After
`CredentialRefreshCommitted` is appended the writer destroys, not deletes (which
is reversible), every version but the current one. A failed destroy is cleanup
debt for the lifecycle worker. Revoking a connection stops refresh and resolve at
once, but destroys all versions only after the provider revoke outcome of
[ADR#0094](./0094-oauth-authorization-protocol.md) Decision 13 is terminal or
its window ends, because that call spends the refresh token.

### 2. Registration secrets are bound to the ProviderApp

A dynamically registered client's `client_secret` and `registration_access_token`
are secrets of the ProviderApp, not of each connection. The first reuses
`ClientSecret`; the second gets a new kind. They are written to custody before
use in the order [ADR#0094](./0094-oauth-authorization-protocol.md) fixes, sealed
and retained like a token set, and destroyed when the app retires. Rotating a
token set never touches them, and public and metadata-document clients hold none.

### 3. A refresh is a write-ahead transaction on the credential stream

`CredentialRefreshDispatched` records the operation id, expected bundle
generation and fence before the provider call. Its settlement
`CredentialRefreshCommitted` records the new generation and `TokenBundle`,
including the explicit `AccessExpiry` evidence and granted authority. The
canonical command, event and bundle definitions live in
`proto/trogonai/secrets/credentials`; this description does not create a second
wire schema.

The broker appends `CredentialRefreshDispatched` with optimistic concurrency
before any HTTP call. The decider admits no second unsettled dispatch, so this
append is the single-flight authority; a NATS KV claim only spreads due work, and
a replica that loses either race waits on the winner's operation. The broker
then calls the token endpoint (with `resource` for MCP), seals and CAS-writes the
bundle at generation plus one, and appends exactly one of Committed,
`CredentialRefreshRejected{reason}` or `CredentialRefreshOutcomeUnknown`. Only a
committed generation reaches an executor.

A recovery worker settles OutcomeUnknown in order. First, custody: a current
bundle carrying the operation id and fence yields the same Committed, idempotent
on the id. Second, provider evidence: only a release documenting stable refresh
tokens or an open reuse grace permits a new operation. Otherwise the credential
is ReconnectRequired (`REFRESH_OUTCOME_LOST`), the family is denied, and provider
revocation becomes cleanup debt. A lease expiry or local timeout never
redispatches, since neither shows the provider did not rotate.

| Failure window | Settlement |
| --- | --- |
| Dispatched, process dies or response lost | OutcomeUnknown, then the order above |
| Bundle written, Committed append lost | Recovery finds the operation id and appends it |
| Committed appended, reply lost | Caller resolves the current generation |
| Claim or lease expired during HTTP | Nothing; the operation stays unsettled |
| Stale worker writes after settlement | CAS and the fence refuse it; attribution kept for cleanup |

| Provider outcome | Result |
| --- | --- |
| `invalid_grant` on the current refresh token | ReconnectRequired; a stale token is never retried |
| 429, or failure before the request left (DNS, connect, TLS) | Rejected, transient; jittered backoff honouring `Retry-After` |
| 5xx or read timeout after the request may have left | OutcomeUnknown, unless the release documents stable tokens |
| Client authentication rejected | ProviderApp Degraded and its refresh paused; user consent is not assumed revoked |

A release may declare whether the provider rotates and any reuse grace. With no
declaration, including every discovered MCP server, the default is rotation with
no grace, the safe assumption for the public clients OAuth 2.1 expects to rotate
or sender-constrain refresh tokens. A refresh never
appends to the Connection stream; only health facts that custody reports
(Degraded, ReconnectRequired, `SCOPES_REDUCED`) cross to it through a reactor,
so a fleet-wide refresh adds no per-connection events.

When a token response grants narrower authority than the connection records,
`CredentialRefreshCommitted` carries it and custody reports it as the health
fact `SCOPES_REDUCED`. That is a health reason on an Active connection, not a
Degraded trigger, and it is the allowed exception to "a refresh never appends".
A wider actual grant invalidates custody with `AUTHORITY_WIDENED`, is never
promoted or resolved, and makes the connection ReconnectRequired. Candidate
material is discarded and cleanup debt records the uncertain provider grant;
rotation may already have occurred, so the old refresh credential is not retried
([ADR#0094](./0094-oauth-authorization-protocol.md) Decision 12). Admission and
the executor read granted authority from the Connection projection, not from the
token set.

### 4. Token sets are sealed under the tenant's sealing series, for every tenant

The KV value is a version field and an
[ADR#0083](./0083-sealed-content-envelope.md) `SealedContent` whose plaintext is
the object of Decision 1. The series is `<tenant>.token-sets`, a store kind
beside streams and buckets, sealing-required from its first write so no cutover
runs. A new layout, `trogon/sealing/token-set/v1`, binds the tenant, `SecretRef`,
purpose and `bundle_generation`. The opener takes the generation it expects from
the credential stream, so a restore that brings back an older sealed version
fails to open instead of being believed. Staged bundles (Decision 12) are sealed
the same way.

For customer managed tenants the alternative is real: leave the value under
OpenBao's barrier, which keeps a destroyed key's refresh tokens alive beyond the
veto. Sealing only those tenants gives two code paths and no rollback binding for
the rest, so every tenant is sealed. The cost is one local AES-256-GCM open per
resolve beyond the OpenBao read already paid, plus at most one provider unwrap
per replica per active generation per minute (leases last 54 to 60 seconds, ADR
0083 Decision 7). A refresh seals once, under a lease acquired before `Dispatched`
with at least twice the request deadline remaining, so a key disabled mid-refresh
cannot strand a rotated token.

| Key event | Resolve | Refresh | Connections |
| --- | --- | --- | --- |
| Customer disables or revokes (reversible) | `KEY_DENIED` | Suspended for the tenant | Unchanged; admission reports the key state; no per-connection events |
| Provider outage | `UNAVAILABLE` | Suspended for the tenant | Unchanged |
| Key restored | Succeeds | Jittered sweep of what is due | Lapsed refresh tokens become ReconnectRequired |
| Key destroyed (irreversible) | `RECONNECT_REQUIRED` | None | ReconnectRequired, `CUSTODY_KEY_DESTROYED`, in jittered batches |

Destruction makes provider-side revocation impossible, so the console's destroy
preflight offers revoke-all first (the call is
[ADR#0094](./0094-oauth-authorization-protocol.md)'s) and records unreachable
revokes as residuals. Orphaned sealed records are inert and cleanup destroys
them. A tier or KEK change re-seals a token set by CAS at the same generation.

### 5. The `secrets/exchange` surface and its callers

The surface is core NATS request and reply, captured by no stream or bucket, so
values stay out of JetStream
([ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 3).
A reply carrying a value is xkey-sealed to the requester (ADR 0083 Decision 1).

```proto
message ResolveForTrustedUseRequest {
  reserved 1, 2;
  reserved secret_ref, grant_ref;
  uint64 min_bundle_generation = 3;
  oneof authorization {
    AttemptRef attempt_ref = 4;
    ProxyBindingRef binding_ref = 8;
    McpGrantRef mcp_grant_ref = 9;
  }
  bool force_refresh = 5;
  AuditCorrelationId correlation_id = 6;
  RequesterXkey requester_xkey = 7;
}
message SealedAccess {
  bytes xkey_sealed_value = 1;
  uint64 bundle_generation = 2;
  google.protobuf.Timestamp valid_until = 3;
}
```

`valid_until` is always finite. For known reported or documented access expiry,
it is the earlier of that expiry less skew and the
[ADR#0049](./0049-revocation-latency-target.md) bound from now. For unknown or
documented non-expiring access, it is the local bound alone. Evicting a local
cache does not expire or revoke the provider grant. The refresh token never
leaves the secrets service.

A resolve is authorized by exactly one proxy binding, protocol-specific MCP
grant or attempt reference. Under `binding_ref`, custody loads the Activated
HTTP_PROXY grant and its exact immutable `ProxyBinding`. The role is fixed by
that binding, so custody selects the secret itself from the connection; the
caller cannot substitute a `SecretRef`, role or upstream host. The verified
caller, tenant, grant audience, Session/attempt, authorization generation,
live standing/permit and accepted audit fact must agree. A generic egress role
has no such authority. The reply xkey must be bound to the authenticated,
registered trusted caller rather than selected by an arbitrary publisher.

`mcp_grant_ref` instead requires MCP_SESSION authority, the pinned canonical
resource and a permitted access role. An attempt reference names the open,
unexpired authorization attempt and its exact staged bundle (Decision 12),
and only the identity-probe worker may use it. The probe runs outside custody;
the broker parses only token, revocation and registration endpoint responses,
never an ordinary provider API response. Unavailable or stale authorization
state fails closed.

`force_refresh` serves a provider authentication rejection. On a 401 the executor
resolves once with the flag and `min_bundle_generation` set to the rejected
generation plus one. Custody refreshes (Decision 3) only if the current
generation is lower, and otherwise returns the current bundle, so concurrent
rejections of one generation cause one refresh. If that refresh fails with
`invalid_grant`, custody records ReconnectRequired. A provider call is retried
only when its response and request semantics establish that replay is safe,
consistent with the HTTP proxy profile. A second attributable 401 is reported to the connections service through
`ReportConnectionHealth` ([ADR#0096](./0096-connection-use-by-agents.md)), and
the connection becomes ReconnectRequired.

| Verbs | Callers |
| --- | --- |
| `ExchangeAuthorizationCode` | The connections service, in the sealed `StartCodeExchange` request that carries the code and state; the broker implements it |
| `OpenTransactionCapsule` | The connections service, at BeginAuthorization |
| `ConsumeTransactionCapsule`, `SignJwt` | The broker only; no NATS publisher while it is a module of the secrets service |
| `RefreshProviderAuthority` | Internal to custody: the refresh scheduler, the recovery worker, and the resolve path when it needs a refresh |
| `ResolveForTrustedUse` | The authenticated connection proxy under its immutable binding, the trusted MCP caller under an MCP_SESSION grant, or the identity-probe worker under its exact attempt/stage |
| `EnsureProviderAccess` | Optional future registered trusted adapters under explicit TOKEN_ISSUANCE authority, refused for person-owned connections |
| `ComputeMac`, `VerifyProviderRequest` | The gateway and the executor (operation verbs, per [ADR#0074](./0074-credential-platform-extraction-boundary.md) Q1) |
| `SubmitMaterial`, `Destroy` | The control-plane management path |

The proxy never signs: `SignJwt` uses app private keys, which are broker
material. An authenticated proxy serving connection traffic may resolve the
access credential only under the matching live grant and tenant binding. It
injects that credential into the provider HTTP request. The generic egress role
in [agent egress proxy proposal](https://github.com/TrogonStack/trogonai/pull/746) (in review) is not automatically a trusted custody caller: the
connection binding, caller identity and proof must authorize this specific use.
The [HTTP proxy profile](../reference/connection-http-proxy.md) specifies the
explicit authenticated HTTPS relay and its grant/request binding. Deployment
isolation and provisioning remain separate implementation decisions. Nothing
here grants an agent access to resolve, refresh or sign. Refresh transactions,
client authentication and token endpoint parsing remain inside custody.

Each verb's publish permission is in the user JWT of its caller class only, and
its subscription in the secrets service's only, per
[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) Decision 4.
The tenant is the account key leading the subject and the principal comes from
the subject ([ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md));
exchange subjects are not exported to a tenant account and callout-admitted users
never hold them. A `PLATFORM` workload must be registered for the trusted caller
role; a wildcard tenant subscription is platform trust, not authorization to
resolve a particular tenant's credential. The service authorizes from its own
state, never the publisher's permission: the connection must belong to the
verified tenant, the binding and audience must match, and the grant must be live,
else `DENIED`.

Optional trusted-adapter issuance is wholly inside `secrets/exchange`.
`EnsureProviderAccessResponse` returns one atomic `SealedAccess` reply to the
adapter's registered xkey, or a typed failure. There is no separate delivery
message, token stream or unresolved cross-package handoff. Its local
`valid_until` is also bounded by grant expiry; it does not shorten the provider
token's actual lifetime. An invocation is consumed once, and a duplicate does
not issue again after a lost reply. This remains a future capability, not a
requirement for normal proxy use.

Exchange RPCs may import custody value types such as `TokenBundle` and typed
endpoint errors. Aggregate packages still do not import another aggregate's
events or state. `AuditCorrelationId` is a shared value in `connections/types`,
so proxy acceptance and custody use can join without coupling aggregates.

### 6. A new OpenBao policy covers runtime token-set writes

`token-set-runtime`, bound to the secrets-service identity, sits on a sibling
subtree, `trogonai/{owner_id}/token-sets/*`, because `control-plane-write`
already overwrites everything under `credentials/*`.

| Path | Capabilities |
| --- | --- |
| `secret/data/trogonai/+/token-sets/*` | create, update, read |
| `secret/metadata/trogonai/+/token-sets/*` | create, read, update |
| `secret/destroy/trogonai/+/token-sets/*` | update |
| `secret/undelete/trogonai/+/token-sets/*` | deny |

The staging path `trogonai/{owner_id}/token-sets/staged/<attempt_id>` (Decision
12) sits under the same glob, so these rows cover its data, metadata and destroy.
`gateway-read` and `control-plane-write` get nothing there.
`lifecycle-worker-cleanup` gains delete, destroy and metadata read, which reaches
abandoned stages, `audit-read` gains metadata read and still denies data, and
`break-glass-admin` gains full control, each with a
`devops/openbao/policies/verify.sh` assertion that includes the staging path.

### 7. The executor holds the access token in memory and closes sessions on revoke

The executor keeps the access token in memory only, never on disk, no longer than
`valid_until`, evicted by revocation fan-out and generation advance per replica
(ADR 0049). On revoke or `CUSTODY_KEY_DESTROYED` it closes the MCP session, as
[ADR#0074](./0074-credential-platform-extraction-boundary.md) Q2 does for the
Discord WebSocket; on a health change it stops issuing requests with the held
token. A session already authenticated stays outside the ADR 0049 bound until
that close lands.

### 8. Refresh load is budgeted and fair

Proactive refresh is due at a random point between 70 and 90 percent of a known
reported or documented finite token lifetime, chosen once per generation, and
runs only for connections with a live grant or recent use. Unknown and
non-expiring access have no lifetime-based proactive due time. A provider
authentication rejection can still request on-demand refresh when refresh
material exists; without it, an attributable dead grant requires reconnect. The scheduler reads project standing live, as admission does
for release and app standing: a connection with custody in an archived project is
refused and its refresh pauses, with no per-connection event, and
`ProjectUnarchived` lifts both. Due work is then swept with jitter through the
same scheduler, and a refresh token that lapsed meanwhile makes the connection
ReconnectRequired. Each ProviderApp caps concurrent token calls (a working
value of 8, lowered by a release's documented limits). On-demand refresh goes
ahead of proactive, and a tenant holds at most half of a platform-owned app's
slots while another waits, so one tenant cannot exhaust a shared provider quota
(`PROVIDER_RATE_LIMITED`). A 429 backs the whole app off. Restarts, restores and
key-restoration sweeps use the same scheduler.

### 9. Refresh and custody are observable

Instruments follow [ADR#0008](./0008-opentelemetry-observability.md), live in the
secrets service's `telemetry/` module, and carry only low-cardinality labels;
drill-down uses traces. Liveness and cache signals are evaluated per replica with
a sample floor, as in ADR 0049; backlog signals derive from the stream. The
thresholds are working values.

| Signal | Alert |
| --- | --- |
| `secrets.token_refresh.attempts{connector,outcome}` | Page when one connector's failures exceed 5 percent over 15 minutes |
| `secrets.token_refresh.unsettled.age` | Page above 15 minutes |
| `secrets.connection.stuck{state,reason}` | Ticket when Degraded lasts 24 hours; page on an `invalid_grant` spike |
| `secrets.provider_app.degraded{connector}` | Page when a platform-owned app stays Degraded an hour |
| `secrets.cleanup.debt.age` | Ticket above an hour; extends the orphan cleanup alert |
| `secrets.capsule.orphaned` | Page when nonzero for 10 minutes |
| `secrets.resolve.failures{category}` | Page on any `INTEGRITY_FAILURE` |

### 10. Restore and key loss mark connections ReconnectRequired, never retry them

A snapshot brings back refresh tokens the provider already rotated away, and a
retry loop on `invalid_grant` can trip reuse detection for the family. After an
OpenBao restore the scheduler stays paused until a sweep compares each opened
bundle's generation with the stream's last committed one. Lower, or an open
Dispatched, means `CUSTODY_RESTORED` and ReconnectRequired, in jittered batches;
if the stream was restored too or cannot be proven untouched, every connection is
marked. Resolve does the same on its own: a generation mismatch fails closed as
stale and queues the mark instead of looping.

Losing the OpenBao seal key ([ADR#0052](./0052-cloud-kms-production-seal.md)) or
a tenant's key is a mass reconnect, and its runbook has a fixed shape: detect,
contain (pause the scheduler), classify platform or tenant scope, choose recovery
from backup plus the KMS key or declare loss, declare `CUSTODY_KEY_LOST` in
batches, notify tenants, record unreachable provider grants as residuals for
tenants to revoke, throttle the reconnect surge through Decision 8's budget, and
verify. The restore steps extend `devops/openbao/runbooks/backup-and-restore.md`;
the key-loss runbook is new beside `unseal-and-key-custody.md`.

### 11. Resolved values are zeroized, failures are typed, and anomalies have a response

A resolved value is `SecretString` as ADR 0083 Decision 7 amends it: an `Arc`
over a zeroizing `Box<str>`, no `Debug`, `Display` or serialization, zeroized on
last drop and on cache eviction; erasure stays best-effort (ADR 0030 Decision 2).

Resolve failures are a closed set of categories carrying no OpenBao path, status
code or provider body, and keeping the semantics of
[ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 6:
unavailability never collapses into denial, and unknown and unauthorized refs
look alike.

| Category | Meaning | Caller retry |
| --- | --- | --- |
| `DENIED` | Caller or grant not permitted, ref unknown, or revoked | No |
| `KEY_DENIED` | The tenant's key refuses | No; the tenant acts |
| `UNAVAILABLE` | OpenBao, key backend or provider unreachable | Bounded backoff |
| `THROTTLED` | Budget or rate exhausted, anomaly throttle included | After the stated delay |
| `RECONNECT_REQUIRED` | Unusable until the user reconnects | No |
| `REFRESH_PENDING` | An unsettled refresh holds the family | Bounded, after delay |
| `INTEGRITY_FAILURE` | Open failed, or generation or AAD mismatch | No; pages |
| `INTERNAL` | Unmapped | No |

Anomaly detection on resolve subjects is structural (ADR 0023). The cache bound
sets the baseline: a legitimate executor resolves a connection about once per
cache lifetime per replica.

| Anomalous when | Response |
| --- | --- |
| One principal resolves a connection more than once per lifetime per replica, sustained | `THROTTLED` for that pair; ticket |
| One principal resolves far more connections than it has live sessions | Throttle to baseline; page |
| Repeated `DENIED` for refs outside any live grant | Cool-down on that principal's resolves; page |
| Any `INTEGRITY_FAILURE` or stale generation | Fail closed; page |
| A resolve for a revoked connection past the ADR 0049 bound | Page naming the stale replica |

The service never revokes connections or disables tenant keys on its own, which
would hand an attacker a denial-of-service lever; containment is a human decision
under `devops/openbao/runbooks/secret-leak-response.md`. Thresholds are working
values set from measured baselines.

### 12. A token bundle is staged under its attempt and promoted by compare-and-set

[ADR#0094](./0094-oauth-authorization-protocol.md) Decision 5 stages the
exchanged bundle before the account is known and marks it usable only after
activation. This decision supplies both markers.

The broker writes the bundle from the code exchange to
`token-sets/staged/<attempt_id>` on the sibling subtree of Decision 6. It is
sealed like a token set (Decision 4), with the attempt id in the `SecretRef`
position and the purpose `staged`, created with `options.cas = 0` and
`max_versions = 1`. The attempt id is the key, so a second exchange under one
attempt cannot overwrite it, and the reconciliation that asks custody whether a
staged bundle exists ([ADR#0094](0094-oauth-authorization-protocol.md) Decision 4) is a metadata read of that path. A
grant cannot resolve a staged bundle and refresh cannot reach it. Only an
attempt reference (Decision 5) opens its access token, and its refresh token never
leaves custody.

The usable marker is promotion. After the connections service commits
ActivateConnection, custody opens the staged bundle, re-seals it under the
connection's `SecretRef`, and CAS-writes it to the connection's token set at
generation 1, then appends the generation to the credential stream and destroys
the staged path. Promotion is idempotent on the attempt id, and a reconciler
finishes it after a crash between activation and promotion. A reconnect stages
beside the live token set and never touches it: the live tokens keep working
until promotion replaces them with the generation it read plus one, so a failed
or abandoned reconnect changes nothing.

A staged bundle is destroyed, every version, when its attempt is aborted, fails
(including `ACCOUNT_ALREADY_CONNECTED`), is lost, or expires. The platform does
not call the provider to revoke a discarded stage. A failed destroy is cleanup
debt, aged by `secrets.cleanup.debt.age` (Decision 9), and an orphan sweep
destroys stages whose attempt is terminal or past its expiry.

## Alternatives Considered

**Leave the token set under OpenBao's barrier for customer managed tenants.**
Rejected: a key destroy leaves the most valuable material live and outside the
veto.

**Seal only customer managed tenants.** Rejected: two code paths, and no
generation binding against a restore for the other tiers.

**Decrypt through the key backend on every resolve.** Rejected for ADR 0083
Decision 7's reason: provider latency and quota on every use.

**A KV lease, secret CAS and epoch as refresh authority, with a per-refresh event
on the Connection.** Rejected: a lease that expires mid-HTTP lets a second replica
replay a rotated token, and the events put fleet-wide volume on the Connection.

**Redispatch on lease expiry or timeout.** Rejected: neither shows the provider
did not rotate, and availability cannot manufacture a lost token.

**Separate secrets for access and refresh tokens, or the mount's default
`max_versions`.** Rejected: torn pairs, and every retained version is a refresh
token and possibly a live access token.

**Refresh inside each executor.** Rejected: the refresh token would leave the
secrets service and single-flight would span every executor.

## Non-Goals

- Connection and ProviderApp state machines
  ([ADR#0092](./0092-third-party-connection-model.md)) and offboarding
  ([ADR#0093](./0093-connection-ownership-and-delegation.md)).
- The authorization flow, capsule contents and provider-side revoke (ADR 0094).
- Admission, grants and HTTP proxy forwarding (ADR 0096).
- Secrets a customer keeps in their own manager
  ([ADR#0079](./0079-external-secret-sources.md)); token sets live in platform
  OpenBao.
- Implementing any of this, or the backup tooling.

## Consequences

- Token sets are lost only through named windows, each with a settlement; an
  ambiguous crash against a rotating provider costs a reconnect.
- A customer managed tenant can shred its token sets by destroying its key, and a
  key outage suspends that tenant's refresh instead of failing each connection.
- Every refresh depends on a sealing lease before dispatch.
- A reconnect never touches the live token set until promotion, and an abandoned
  attempt leaves only a staged bundle that cleanup destroys.
- Amends ADR 0083 Decision 1: KV values stay outside the veto except token sets
  and registration secrets. It adds the `token-set` layout to Decision 3 and the
  store kind to Decision 4, and Decision 8 now erases them too.
- Clarifies [ADR#0033](./0033-key-custody-product-model.md) Decisions 6 and 7: KV
  v2 stays the store, and the token-set series is a sealing series, not a key
  purpose. A stricter reading of Decision 6 needs an amendment.
- Amends ADR 0074: a policy beside those of Decision 3, the broker and token sets
  in Decision 1's scope, and a sibling subtree beside Q4's path convention.
- Resolves the wrapper, failure taxonomy and anomaly decisions in [Secret
  Management](../architecture/secret-management.md), which updates on acceptance.
- Changes `devops/openbao`: a policy, `verify.sh` assertions, runbooks and
  `alerts.md`.

## References

- [ADR#0008](./0008-opentelemetry-observability.md),
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md),
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md),
  [ADR#0033](./0033-key-custody-product-model.md),
  [ADR#0047](./0047-event-sourced-credential-metadata.md),
  [ADR#0049](./0049-revocation-latency-target.md),
  [ADR#0052](./0052-cloud-kms-production-seal.md),
  [ADR#0074](./0074-credential-platform-extraction-boundary.md),
  [ADR#0079](./0079-external-secret-sources.md),
  [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md),
  [ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md),
  [ADR#0083](./0083-sealed-content-envelope.md)
- [ADR#0092](./0092-third-party-connection-model.md),
  [ADR#0093](./0093-connection-ownership-and-delegation.md),
  [ADR#0094](./0094-oauth-authorization-protocol.md),
  [ADR#0096](./0096-connection-use-by-agents.md)
- [Secret Management](../architecture/secret-management.md),
  [Key States](../architecture/key-states.md),
  [OpenBao](../glossary/openbao)
- [RFC 9700, OAuth 2.0 Security Best Current Practice](https://www.rfc-editor.org/rfc/rfc9700.html)
- [OpenBao KV v2 API](https://openbao.org/api-docs/secret/kv/kv-v2/)
- [MCP authorization, 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)

## Operational defaults

The [security profile](../reference/connector-security-profile.md) fixes sealing
for every tenant's token material within ADR0083's existing scope, immediate
customer key-veto behavior, tenant-isolated credential-bearing workers, the broker
inside the minimal secrets service and reviewed-release-only refresh exceptions.
The proposed connector stream-class refinements are linked from their source
ADRs; no accepted ADR status is changed here.

A vault remains a policy grouping within the tenant in this milestone, not an
additional cryptographic boundary, routing ancestor or independent lifecycle
aggregate. Credential lifecycle, custody project and authorization stay explicit.
A future vault resource/lifecycle requires a separate contract and migration.
