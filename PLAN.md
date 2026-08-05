# Credential Vault And API Key Platform Plan

## Purpose

This plan turns the current secret-store and API-key design notes into an
implementation roadmap.

The goal is to build a Vercel-like credential management experience for
Trogonai while keeping raw credential material out of the normal application
database.

The plan is grounded in:

- `SECRET_STORE.md` for provider credentials, OpenBao, lifecycle sagas,
  runtime resolution, cleanup, and reconciliation;
- `API_KEY.md` for Trogonai-issued API keys, verifier-only bearer keys,
  Coinbase-style signed keys, and Unkey-style management concepts.

## Status

This document now tracks only remaining work. The first implementation slice
is already merged into `trogon-gateway` under `credential::` (event-sourced
credential decider, command handler, runtime handler, recovery worker,
checkpointed runtime projection refresh) and `secret_store::` (segregated
store traits with static, in-memory, and OpenBao adapters), plus the internal
admin management API under `/-/credentials` with scoped idempotency, the ten
runtime-backed webhook routers, the Discord runtime bot token path, and the
protobuf contracts under `proto/trogonai/gateway/credentials`. Completed work
was removed from this file; the code and its tests are the reference for what
exists.

## Production Follow-Ups From The Implemented Slice

```text
decide whether management idempotency records move from NATS KV to the future
control-plane database
  -> tune snapshot frequency and retention after production stream metrics exist
  -> write production OpenBao auth, HA, backup, cleanup, and alert runbooks
```

## Direction

Use this split:

```text
Provider credentials
  -> raw material needed later by Trogonai
  -> OpenBao
  -> DB stores metadata, refs, status, fingerprints, policies

Trogonai bearer API keys
  -> authenticate callers to Trogonai
  -> raw key shown once
  -> DB stores verifier digest and metadata

Trogonai signed API keys
  -> high-authority caller authentication
  -> caller keeps private key
  -> DB stores public verification material and metadata
```

The enterprise-ready API key direction is:

```text
Unkey-style control plane
  -> keyspaces, identities, roles, permissions, rate limits,
     rerolling, revocation, audit, analytics

Coinbase-style authentication proof
  -> asymmetric key pair, short-lived signed request token,
     method/host/path binding, nonce, expiry, optional IP allowlist
```

OpenBao is the internal credential backend. It is not the public product API.

## Non-Goals For The First Version

- Do not build a general-purpose OAuth authorization server first.
- Do not require FAPI certification first.
- Do not require mutual TLS for every customer first.
- Do not store customer caller private keys in OpenBao by default.
- Do not expose OpenBao paths, mounts, tokens, or policies through the public
  API.
- Do not put raw provider secrets in the application database.
- Do not make every runtime request call OpenBao.
- Do not make Valkey the source of truth for secrets.

## Product Model

The user-facing model should be simple:

```text
Credential vault
  -> groups credentials by owner/environment/use

Credential
  -> metadata and lifecycle around one logical credential

Credential version
  -> one stored version of credential material

Credential ref
  -> stable reference used by runtime services

API keyspace
  -> namespace for Trogonai-issued API keys

API key
  -> bearer or signed caller authentication credential
```

The UI should never expose saga, OpenBao, mount, path, or policy language.
Users should see product states:

```text
draft
saving
ready
failed
needs_secret_resubmission
rotation_pending
revoked
cleanup_pending
```

## Architecture Boundaries

### Application Database

The application database stores:

- owners, workspaces, identities, and authorization scope;
- vault metadata;
- integration metadata;
- credential metadata;
- credential refs;
- credential lifecycle state;
- fingerprints;
- delivery policy;
- idempotency records;
- operation records;
- audit facts;
- API-key verifier digests;
- signed-key public verification material.

The application database must not store:

- raw provider API keys;
- OAuth refresh tokens;
- webhook signing secrets;
- bot tokens;
- raw Trogonai bearer API keys;
- caller private keys;
- generated one-time private keys;
- plaintext secret values inside saga, workflow, outbox, or retry payloads.

### OpenBao

OpenBao stores:

- provider API keys;
- OAuth refresh tokens;
- webhook signing secrets;
- bot tokens;
- decryptable runtime credentials;
- verifier peppers when needed;
- issuer signing keys when needed;
- certificate authority material when needed.

OpenBao should be read:

- on credential creation or rotation;
- on runtime cache miss;
- on gateway/session startup or reconnect;
- during explicit refresh;
- during cleanup or reconciliation.

OpenBao should not be read for every webhook or every ordinary runtime request.

### Gateway And Runtime Services

The gateway receives metadata projections and resolves only active
`CredentialRef` values when authorized.

Runtime services should receive typed values:

```text
GitHubWebhookSecret
SlackSigningSecret
DiscordBotToken
ProviderApiKey
OAuthRefreshToken
```

They should not receive arbitrary OpenBao paths from callers.

## Phase 0: Finalize Buildable Specs

All six deliverables are drafted as sections of
`CREDENTIAL_PLATFORM_SPEC.md`, grounded against the shipped gateway code and
carrying every open question inline as a DECISION NEEDED block with a
recommendation. The phase completes when those decisions are ratified and
the sections split into their own files:

- `CREDENTIAL_LIFECYCLE.md`
- `API_CONTRACTS.md`
- `AUTHORIZATION_MATRIX.md`
- `OPENBAO_OPERATIONS.md`
- `RUNTIME_PROJECTION.md`
- `UI_ACCEPTANCE.md`

### Required Decisions

Owner boundary, metadata backend, path convention, cache TTL, and latency
target are decided in ADR#0042..0045. The signed-key algorithm and the
signed-first caller authentication posture are decided in ADR#0046.

- First OpenBao auth method per service. The prototype uses a static dev
  token; that is not a production answer.
- First supported credential kinds.
- First API keyspaces.

### Acceptance Criteria

- Every state has allowed transitions.
- Every API write command has an idempotency rule.
- Every endpoint says whether it can ever return plaintext.
- Every credential kind says where raw material lives.
- Every runtime service identity has explicit permissions.
- Every failure mode has a user-visible state and recovery path.

## Phase 1: Domain Model And State Machines

The credential-scoped value objects and the version lifecycle decider already
exist under `credential::commands::domain`. What remains is the broader domain
model around them.

### Implement Remaining Value Objects

Define rich types instead of primitive strings:

```text
OwnerId          (project id per ADR#0042; today only a credential-scoped
                  CredentialOwnerId exists)
WorkspaceId      (collapses into the project id per ADR#0042)
VaultId
CredentialVersionId
CredentialPath   (today path building lives inside the OpenBao adapter)
ApiKeyId
ApiKeyspaceId
ApiKeyKind
IdentityId
OperationId
```

### Complete The Credential State Machines

The decider covers pending/active/previous/revoked/expired version states
with write, rotation, and revoke transitions, plus the destroy saga states
(destroy_requested, destroyed, cleanup_failed) with an admin destroy route
and idempotent retry. Remaining states:

```text
CredentialVersionState (additions)
  resubmission_required   (blocked on the write_failed mapping decision)
  revocation_requested    (belongs with Phase 6 async cleanup)

CredentialState (not built)
  draft
  ready
  failed
  disabled
  archived
  deleted

IntegrationState (not built)
  pending
  active
  failed
  disabled
  reconnect_required
  archived
  deleted

OperationState (not built)
  accepted
  running
  succeeded
  failed
  conflict
  expired
```

### Define API Key State Machines

Nothing exists for these yet:

```text
ApiKeyState
  active
  revoked
  expired
  reroll_pending
  disabled

SignedPublicKeyState
  active
  previous
  revoked
  expired
```

### Acceptance Criteria

- Invalid transitions are impossible in domain code.
- State transitions emit audit facts.
- State transitions can be retried safely.
- State transition tests cover success, conflict, and failure paths.

## Phase 2: SecretStore Contract

Complete. The segregated store traits are the single contract shape (the
unused unified trait was deleted), and `destroy(ref, reason)` exists across
the adapters with a `destroyed` terminal status backed by the OpenBao KV v2
destroy endpoint. The destroy lifecycle saga (destroy_requested, destroyed,
cleanup_failed) and the admin destroy route now drive it end to end; the
remaining destroy-adjacent work is Phase 6 async cleanup.

## Phase 3: Persistence And Idempotent Operations

Scoped idempotency for create, rotate, and revoke already works through a
protobuf NATS KV ledger in the gateway. The remaining work is the persistence
model the plan actually calls for:

### Work

- Add persistence for vaults, credentials, versions, operations, idempotency
  records, and audit facts. None of these exist outside the event stream; the
  gateway has no database dependency and no vault, operation, or audit-fact
  concept at all.
- Add pending-operation caps. The only cap today is the incidental
  one-pending-write-per-credential rule from the aggregate design.
- Add TTL handling for pending operations. Today the only expiries are the
  blanket 24h idempotency bucket age and the recovery worker's stuck window.
- Extend the idempotency record to the full contract shape below; the current
  KV record carries only fingerprint, status, and response.
- Decide whether idempotency records stay in NATS KV or move to the
  control-plane database.

### Idempotency Contract

The client supplies an opaque key. The server supplies the scope.

```text
IdempotencyRecord
  owner_id
  workspace_id       (project id per ADR#0042)
  command_namespace
  target_resource_id
  idempotency_key
  request_fingerprint
  operation_id
  resource_id
  response_snapshot
  status
  created_by_actor_id
  created_at
  expires_at
```

Recommended uniqueness:

```text
unique(owner_id, command_namespace, idempotency_key)
```

For targeted operations:

```text
unique(owner_id, command_namespace, target_resource_id, idempotency_key)
```

The contract:

```text
same owner + same namespace + same key + same request fingerprint
  -> return same operation/resource/status

same owner + same namespace + same key + different request fingerprint
  -> idempotency conflict

different owner + same key
  -> unrelated operation
```

### Acceptance Criteria

- Retrying create ten times creates one credential intent.
- Retrying rotate ten times creates one pending version.
- Same idempotency key with different body returns conflict.
- Two owners can use the same raw idempotency key without collision.
- Response snapshots never contain raw secrets or one-time key material.

## Phase 4: Credential Management API

Nothing public exists yet. The only surface today is the internal
admin-token-gated per-source API under `/-/credentials`; the vault-shaped
public API below is unbuilt, and there is no vault, operation, or
resubmit-secret concept behind it.

### Endpoints

Paths are parent-scoped resource names rooted at the project, following
ADR#0042 section 3.

```text
POST   /v1/projects/{project}/credential-vaults
GET    /v1/projects/{project}/credential-vaults
GET    /v1/projects/{project}/credential-vaults/{vault_id}
PATCH  /v1/projects/{project}/credential-vaults/{vault_id}
POST   /v1/projects/{project}/credential-vaults/{vault_id}/archive
POST   /v1/projects/{project}/credential-vaults/{vault_id}/restore

POST   /v1/projects/{project}/credentials
GET    /v1/projects/{project}/credentials
GET    /v1/projects/{project}/credentials/{credential_id}
PATCH  /v1/projects/{project}/credentials/{credential_id}
POST   /v1/projects/{project}/credentials/{credential_id}/rotate
POST   /v1/projects/{project}/credentials/{credential_id}/resubmit-secret
POST   /v1/projects/{project}/credentials/{credential_id}/revoke
POST   /v1/projects/{project}/credentials/{credential_id}/archive
POST   /v1/projects/{project}/credentials/{credential_id}/delete

GET    /v1/projects/{project}/operations/{operation_id}
```

### Response Rules

Metadata-only by default:

```text
allowed
  -> ids
  -> display names
  -> state
  -> kind
  -> source
  -> fingerprint
  -> allowed hosts
  -> allowed runtime services
  -> last used metadata
  -> operation id

forbidden
  -> raw secret
  -> provider token
  -> OAuth refresh token
  -> webhook signing secret
  -> bot token
```

### Error Codes

```text
validation_failed
permission_denied
idempotency_conflict
pending_operation_exists
credential_not_ready
secret_write_failed
needs_secret_resubmission
host_not_allowed
runtime_service_not_allowed
secret_store_unavailable
provider_registration_failed
cleanup_pending
```

### Acceptance Criteria

- Create returns an operation id.
- Retry returns the same operation id.
- Read never returns plaintext.
- Failed create has a clear status and recovery action.
- Lost secret material requires resubmission.
- If OpenBao has the value, reconciliation can continue without user
  resubmission.

## Phase 5: OpenBao Hardening (Remaining)

The OpenBao adapter exists behind the store traits and has a Testcontainers
round-trip test. The operational contract around it does not exist:

### Work

- Ratify the path convention as a published decision (see Phase 0). External
  callers must never provide arbitrary OpenBao paths.
- Define and configure service auth methods. Today everything uses a static
  dev root token.
- Define policies.

### Metadata Convention

Every OpenBao write now records path-level custom metadata with the fields
that exist today: owner_id, credential_id, credential_kind, current_version,
created_at. Remaining fields wait on their concepts:

```text
workspace_id       (project id per ADR#0042)
integration_id     (needs integration records)
operation_id       (needs Phase 3 operation records)
credential_version_id
  -> KV v2 custom metadata is path-level, not per-version; per-version
     attribution needs its own convention
```

### Policies

Separate policies for:

```text
control_plane_write
gateway_read
lifecycle_worker_cleanup
audit_read
break_glass_admin
```

No policy files exist anywhere in the repo yet (no HCL, no Terraform).

### Acceptance Criteria

- Control plane can write credential material.
- Gateway can read only active refs it is authorized to resolve.
- Cleanup worker can revoke or destroy only scoped paths.
- Audit roles cannot read raw secret values.
- OpenBao paths are deterministic and reconcilable.

## Phase 6: Saga, Cleanup, And Reconciliation

The gateway handler already performs the immediate write/rotate/revoke saga
against OpenBao with a recovery worker for stuck activations. The plan-level
saga with a database intent, an outbox, and split cleanup is unbuilt: there is
no outbox anywhere, no resubmission flow, no orphan cleanup, no tombstones,
and revoke currently deletes from OpenBao synchronously in the same request.

### Create Saga

```text
1. Authorize command.
2. Create scoped idempotency record.
3. Create DB credential/version intent as pending_secret_write.
4. Write raw secret to OpenBao.
5. Mark version active.
6. Emit outbox event.
7. Gateway refreshes projection.
```

### Rotation Saga

```text
1. Keep current active version active.
2. Create new pending version.
3. Write new secret to OpenBao.
4. Validate if provider supports validation.
5. Promote new version to active.
6. Mark old version previous or revoked.
7. Emit outbox event.
```

### Cleanup Rules

Logical cleanup first:

```text
DB state prevents runtime use
  -> revoked / disabled / deleted
```

Physical cleanup later:

```text
OpenBao revoke or destroy
provider revocation when supported
tombstone retention
```

### Reconciliation Jobs

DB -> Gateway convergence already exists through the checkpointed projection
refresh worker. The other three jobs are unbuilt:

```text
DB -> OpenBao
  -> expected secret exists and has expected metadata

OpenBao -> DB
  -> every managed secret has a DB owner or becomes orphan cleanup

Provider -> DB
  -> provider-side revocation or disconnect reflected when possible
```

### Acceptance Criteria

- Every midway failure has a test.
- OpenBao write success plus DB activation failure can be reconciled.
- DB pending plus no OpenBao secret expires to resubmission.
- Cleanup is idempotent.
- Runtime safety does not depend on physical cleanup completing immediately.

## Phase 7: Runtime Projection And Cache (Remaining)

The projection, its checkpointed refresh, event-driven invalidation,
per-`CredentialRef` versioning, and fail-closed behavior on revoked or
disabled credentials all exist. What remains:

### Projection Shape

Extend the current per-integration projection with the policy fields the plan
requires:

```text
RuntimeCredentialProjection (missing fields)
  workspace_id       (project id per ADR#0042)
  allowed_hosts
  allowed_runtime_services
  injection_locations
  cache_policy
```

### Cache Rules

- TTL with deterministic per-key jitter exists (default 300s ttl, 30s
  jitter); expose the policy through configuration once the Phase 0 cache
  TTL decision is ratified.
- Invalidate on outbox projection event once the outbox exists.
- Decide fallback behavior for OpenBao outage per credential kind.

### Acceptance Criteria

- Revocation reaches gateway within the target latency, and that latency is
  measured.
- Cache miss behavior is observable.

## Phase 8: API Key Platform

Nothing is implemented; `API_KEY.md` is design-only and no bearer key, signed
key, keyspace, or `ApiPrincipal` code exists anywhere in the repo. Signed
mode is the strongly recommended default and the primary build target per
ADR#0046; bearer is the policy-bounded compatibility tier.

### Signed Keys

Implement Coinbase-style signed keys as the recommended mode:

```text
api_key.public_key.add
api_key.public_key.revoke
api_key.verify_signed_request
```

Rules:

- client-generated key pairs only; the platform never holds private keys;
- Ed25519 default, ES256 accepted (ADR#0046);
- DB stores public key and fingerprint;
- signed token binds method, host, path, time, and nonce;
- replay cache and bounded clock-skew tolerance on the platform side;
- root/management keyspaces are signed-only.

### Bearer Keys

Implement Unkey-style bearer keys as the compatibility tier:

```text
api_key.create
api_key.reroll
api_key.revoke
api_key.verify
```

Rules:

- raw key shown once (ADR#0044);
- DB stores verifier digest;
- verifier pepper lives outside the API-key table;
- list/read responses are metadata-only;
- lost one-time response requires reroll by default;
- keyspace policy can disallow bearer issuance entirely (ADR#0046).

### Authorization Result

Both modes should return an `ApiPrincipal`:

```text
ApiPrincipal
  owner_id
  identity_id
  key_id
  keyspace_id
  scopes
  roles
  allowed_vaults
  allowed_integrations
  allowed_environments
```

### Acceptance Criteria

- Bearer verification is constant-time.
- Signed verification checks signature, expiry, nonce, method, host, and path.
- API keys never directly return raw provider credentials.
- Root keys cannot delegate more authority than they have.
- Rate limits can attach to keys, identities, owners, or routes.

## Phase 9: UI And Client Experience

Nothing is implemented. The console app is a single-route scaffold with no
data layer, and the credentials proto packages define no RPC services a
browser client could call, so this phase also depends on defining that public
API surface (Phase 4).

### Credential Vault List

Show:

```text
name
environment
kind
status
fingerprint
last used
allowed hosts
actions
```

Actions:

```text
add
rotate
resubmit secret
revoke
archive
delete
view audit
```

### Add Credential Flow

```text
1. User enters metadata.
2. User enters secret value.
3. Client generates idempotency key.
4. Client submits once.
5. Client clears plaintext from memory.
6. UI shows operation status.
7. UI polls operation id.
```

### Failure UX

```text
secret_write_failed
  -> show retry if request still active
  -> otherwise show resubmit secret

needs_secret_resubmission
  -> ask for secret again

cleanup_pending
  -> show revoked/disabled for runtime safety
  -> show cleanup status only in details

lost one-time API key response
  -> show key exists but value cannot be recovered
  -> offer reroll
```

### Acceptance Criteria

- User never sees OpenBao internals.
- User never sees raw secret after creation.
- UI cannot create duplicate pending records through retry.
- Rotation does not break the old active credential until the new version is
  ready.
- Failed operations provide a clear next action.

## Phase 10: Operations And Runbooks

None of these exist. Only the local dev compose README is written, and no
alert or monitor definitions exist for the OTel counters the gateway already
emits.

### Required Runbooks

- OpenBao dev setup.
- OpenBao production HA setup.
- Unseal and key custody.
- Backup and restore.
- Audit log export and review.
- Secret leak response.
- Stuck `pending_secret_write`.
- Orphan OpenBao secret cleanup.
- Cleanup worker failure.
- Gateway projection miss.
- Provider revocation failure.
- Break-glass access.

### Alerts

Alert on:

- repeated OpenBao write failures;
- OpenBao read failure rate;
- stuck pending credentials;
- orphan cleanup backlog;
- gateway projection lag;
- cache miss spike;
- denied host spike;
- suspicious API-key verification failures;
- signed-request replay attempts.

The recovery worker counters under `gateway.credential.recovery.*` are the
first signals to wire up:

```text
stuck_reports increases
  -> page or route to the operator owning OpenBao and lifecycle recovery

failed_recovery passes continue increasing while the checkpoint stays pinned
  -> investigate activation append failures or OpenBao metadata reads

retry_delayed remains true longer than the stuck-after policy
  -> use /-/credentials/recovery/status to confirm failure age and retry window
```

### Acceptance Criteria

- Restore drill proves OpenBao and DB can be reconciled.
- Lost gateway event can be recovered by projection refresh.
- Break-glass access is audited.
- Runbooks explain customer-visible impact and recovery.

## Phase 11: Testing Strategy (Remaining)

The implemented slice already carries unit tests for value objects, state
transitions, and idempotency conflicts, plus integration tests for the
static, in-memory, and OpenBao adapters, projection refresh, and cache
invalidation. The remaining categories:

### Unit Tests

- verifier digest construction (blocked on Phase 8);
- signed-request verification (blocked on Phase 8);
- delivery policy validation (blocked on Phase 7 projection fields);
- allowed-host matching (blocked on Phase 7 projection fields).

### Integration Tests

- create saga with DB intent and outbox (blocked on Phase 6);
- cleanup worker (does not exist yet; the recovery worker covers stuck
  activations, not orphaned-secret cleanup).

### Failure Injection Tests

Still uncovered:

```text
DB intent write fails
OpenBao write outcome unknown
DB activation succeeds but outbox publish fails
cleanup worker fails
provider revocation fails
client loses one-time API key response
```

### Security Tests

- no plaintext in outbox (blocked on Phase 6 outbox);
- no plaintext in logs;
- no plaintext in traces;
- no plaintext in metrics;
- denied host cannot be bypassed;
- unauthorized service identity cannot resolve credentials;
- signed request replay is rejected.

### Load Tests

None exist yet:

- gateway cache hit rate;
- gateway cache miss pressure on OpenBao;
- rotation invalidation latency;
- revocation latency;
- idempotency ledger contention;
- OpenBao read/write throughput.

## Milestone Order

Milestone 1 (internal contract: credential-scoped value objects, store
traits, static and in-memory adapters, preserved gateway behavior) is complete
and removed. The broader value objects still owed are listed in Phase 1.

### Milestone 2: Credential Metadata And Operations (Remaining)

- DB metadata model.
- operation records;
- full idempotency record shape (KV ledger exists for the current commands);
- credential/integration/operation state machines;
- public management API skeleton.

Exit criteria:

- create/rotate/revoke commands are metadata-only;
- retries converge on one operation;
- pending-operation caps exist.

### Milestone 3: OpenBao Write Path (Remaining)

The OpenBao adapter, immediate create/rotate saga, and create-failure recovery
already exist. Remaining:

- create saga with DB intent and outbox;
- failure injection for every create midway point.

Exit criteria:

- raw provider credentials are stored in OpenBao;
- DB only stores refs and metadata;
- failure injection covers every create midway point.

### Milestone 4: Runtime Delivery (Remaining)

Projection, cache with TTL and jitter, invalidation, fail-closed
revocation, and the revocation-to-invalidation latency measurement
(`gateway.credential.revocation.latency`) exist. Remaining:

- ratify the latency target and alert on the measurement.

### Milestone 5: Product UI

- vault list;
- add credential modal;
- rotate flow;
- revoke/archive flow;
- operation status view;
- failure/resubmission UX.

Exit criteria:

- UI acceptance scenarios match the screenshots;
- retry does not create duplicate pending records;
- failure states have clear user actions.

### Milestone 6: API Key Platform

- bearer API keys;
- verifier-only storage;
- keyspaces and identities;
- permissions and roles;
- rate limits;
- signed public-key registration;
- signed request verification.

Exit criteria:

- normal developer keys work with bearer mode;
- high-authority keys work with signed mode;
- raw bearer keys are shown once;
- signed private keys are not stored by default.

### Milestone 7: Operations Readiness

- OpenBao HA profile;
- backup/restore;
- audit log handling;
- incident runbooks;
- alerting;
- load tests.

Exit criteria:

- restore drill passes;
- leak response is documented;
- stuck saga runbook is tested;
- revocation latency target is measured.

## Implementation Defaults

Unless later decisions override these, use:

```text
credential backend
  -> event stream as the metadata source of truth (ADR#0043)
  -> OpenBao for raw provider credential material

API key model
  -> signed keys strongly recommended for all callers (ADR#0046)
  -> bearer keys as the policy-bounded compatibility tier
  -> root/management keyspaces are signed-only

signed key default
  -> Coinbase-style JWT request token
  -> client-generated key pair only; the platform never holds private keys
  -> Ed25519 default, ES256 accepted for compatibility (ADR#0046)

idempotency
  -> scoped by owner/workspace and command namespace
  -> target resource included for targeted operations
  -> metadata-only replay snapshots

runtime
  -> projection plus cache
  -> OpenBao on cache miss/refresh, not every request

cleanup
  -> logical cleanup first
  -> physical cleanup async and idempotent
```

## Decisions Still Needed

- Which OpenBao auth method should each service use?
- Which credential kinds ship in the first UI?
- Which providers get first-class validation?
- What default idempotency TTL should be.
- What default pending credential TTL should be.

## Definition Of Done

The overall project is not done until:

- raw provider credentials are not in the application database;
- public API responses are metadata-only after one-time display;
- idempotent retries cannot create duplicate pending records;
- OpenBao paths are generated from validated domain values;
- every midway saga failure has a recovery path;
- gateway runtime resolution is authorized and cached;
- cleanup is idempotent and observable;
- API keys are split between verifier-only bearer keys and signed keys;
- signed keys do not require Trogonai to store caller private keys;
- UI states hide distributed-system details from users;
- runbooks exist for stuck, leaked, orphaned, and missed-projection scenarios;
- tests prove redaction, authorization, retry, cleanup, rotation, and
  revocation behavior.
