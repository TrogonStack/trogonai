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

### Deliverables

- `CREDENTIAL_LIFECYCLE.md`
- `API_CONTRACTS.md`
- `AUTHORIZATION_MATRIX.md`
- `OPENBAO_OPERATIONS.md`
- `RUNTIME_PROJECTION.md`
- `UI_ACCEPTANCE.md`

These can begin as sections in one file, but they should become separate files
before implementation grows.

### Required Decisions

- Owner boundary: workspace, organization, project, tenant, or user.
- First credential metadata backend: Postgres, NATS KV, or existing control
  plane store.
- First OpenBao path convention.
- First OpenBao auth method per service.
- First supported credential kinds.
- First API keyspaces.
- First signed-key algorithm.
- First default cache TTL and revocation latency target.

### Acceptance Criteria

- Every state has allowed transitions.
- Every API write command has an idempotency rule.
- Every endpoint says whether it can ever return plaintext.
- Every credential kind says where raw material lives.
- Every runtime service identity has explicit permissions.
- Every failure mode has a user-visible state and recovery path.

## Phase 1: Domain Model And State Machines

### Implement Value Objects

Define rich types instead of primitive strings:

```text
OwnerId
WorkspaceId
VaultId
CredentialId
CredentialVersionId
CredentialRef
CredentialKind
CredentialSource
CredentialFingerprint
CredentialPath
CredentialVersion
ApiKeyId
ApiKeyspaceId
ApiKeyKind
IdentityId
OperationId
IdempotencyKey
RequestFingerprint
```

### Define Credential State Machines

Minimum states:

```text
CredentialVersionState
  pending_secret_write
  active
  previous
  secret_write_failed
  resubmission_required
  revocation_requested
  revoked
  destroy_requested
  destroyed
  cleanup_failed

CredentialState
  draft
  ready
  failed
  disabled
  archived
  deleted

IntegrationState
  pending
  active
  failed
  disabled
  reconnect_required
  archived
  deleted

OperationState
  accepted
  running
  succeeded
  failed
  conflict
  expired
```

### Define API Key State Machines

Minimum states:

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

## Phase 2: SecretStore Contract And Static Adapter

### Work

- Keep current TOML/env behavior.
- Introduce the `SecretStore` trait around existing source hydration.
- Add a static config adapter for current resolved secrets.
- Add an in-memory test adapter.
- Keep source handlers receiving typed secrets.

### Contract Shape

```text
SecretStore
  write(ref, secret, metadata)
  read(ref, purpose)
  revoke(ref, reason)
  destroy(ref, reason)
  metadata(ref)
```

The exact names can follow the existing Rust style, but the domain separation
should stay intact:

```text
secret values
  -> only through ports/adapters

domain records
  -> refs, metadata, status, policies
```

### Acceptance Criteria

- Existing gateway config behavior still works.
- Static config adapter passes current tests.
- In-memory adapter supports deterministic unit tests.
- No source handler needs to know about OpenBao.

## Phase 3: Persistence And Idempotent Operations

### Work

- Add persistence for vaults, credentials, versions, operations, idempotency
  records, and audit facts.
- Add scoped idempotency for every write command.
- Add pending-operation caps.
- Add TTL handling for pending operations.

### Idempotency Contract

The client supplies an opaque key. The server supplies the scope.

```text
IdempotencyRecord
  owner_id
  workspace_id
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

### Endpoints

```text
POST   /v1/credential-vaults
GET    /v1/credential-vaults
GET    /v1/credential-vaults/{vault_id}
PATCH  /v1/credential-vaults/{vault_id}
POST   /v1/credential-vaults/{vault_id}/archive
POST   /v1/credential-vaults/{vault_id}/restore

POST   /v1/credentials
GET    /v1/credentials
GET    /v1/credentials/{credential_id}
PATCH  /v1/credentials/{credential_id}
POST   /v1/credentials/{credential_id}/rotate
POST   /v1/credentials/{credential_id}/resubmit-secret
POST   /v1/credentials/{credential_id}/revoke
POST   /v1/credentials/{credential_id}/archive
POST   /v1/credentials/{credential_id}/delete

GET    /v1/operations/{operation_id}
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

## Phase 5: OpenBao Adapter

### Work

- Add OpenBao adapter behind `SecretStore`.
- Define path conventions.
- Define metadata conventions.
- Define service auth methods.
- Define policies.
- Add integration tests with a local OpenBao instance if practical.

### Path Convention

Initial convention:

```text
kv/trogonai/{owner_id}/credentials/{credential_id}/versions/{version_id}
```

The exact mount can change, but it must be generated from validated domain
values. External callers must never provide arbitrary OpenBao paths.

### Metadata Convention

Every OpenBao write should include non-secret metadata:

```text
owner_id
workspace_id
integration_id
credential_id
credential_version_id
credential_kind
operation_id
created_at
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

### Acceptance Criteria

- Control plane can write credential material.
- Gateway can read only active refs it is authorized to resolve.
- Cleanup worker can revoke or destroy only scoped paths.
- Audit roles cannot read raw secret values.
- OpenBao paths are deterministic and reconcilable.

## Phase 6: Saga, Cleanup, And Reconciliation

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

```text
DB -> OpenBao
  -> expected secret exists and has expected metadata

OpenBao -> DB
  -> every managed secret has a DB owner or becomes orphan cleanup

DB -> Gateway
  -> active projection version reached gateway

Provider -> DB
  -> provider-side revocation or disconnect reflected when possible
```

### Acceptance Criteria

- Every midway failure has a test.
- OpenBao write success plus DB activation failure can be reconciled.
- DB pending plus no OpenBao secret expires to resubmission.
- Cleanup is idempotent.
- Runtime safety does not depend on physical cleanup completing immediately.

## Phase 7: Runtime Projection And Cache

### Projection Shape

```text
RuntimeCredentialProjection
  owner_id
  workspace_id
  integration_id
  credential_ref
  credential_kind
  credential_version
  state
  allowed_hosts
  allowed_runtime_services
  injection_locations
  cache_policy
  updated_at
```

### Cache Rules

- Cache by `CredentialRef` and version.
- Add TTL and jitter.
- Invalidate on outbox projection event.
- Refresh on reconnect.
- Fail closed on revoked or disabled state.
- Decide fallback behavior for OpenBao outage per credential kind.

### Acceptance Criteria

- Gateway does not read OpenBao on every webhook request.
- Revocation reaches gateway within the target latency.
- Cache miss behavior is observable.
- Cache entries are versioned so rotation cannot reuse stale values silently.
- Projection refresh is idempotent.

## Phase 8: API Key Platform

### Bearer Keys

Implement Unkey-style ordinary bearer keys:

```text
api_key.create
api_key.reroll
api_key.revoke
api_key.verify
```

Rules:

- raw key shown once;
- DB stores verifier digest;
- verifier pepper lives outside the API-key table;
- list/read responses are metadata-only;
- lost one-time response requires reroll by default.

### Signed Keys

Implement Coinbase-style signed high-authority keys:

```text
api_key.public_key.add
api_key.public_key.revoke
api_key.verify_signed_request
```

Rules:

- prefer client-generated key pairs;
- DB stores public key and fingerprint;
- private key is not stored by default;
- signed token binds method, host, path, time, and nonce;
- root/management keys should prefer signed mode.

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

### Acceptance Criteria

- Restore drill proves OpenBao and DB can be reconciled.
- Lost gateway event can be recovered by projection refresh.
- Break-glass access is audited.
- Runbooks explain customer-visible impact and recovery.

## Phase 11: Testing Strategy

### Unit Tests

- value object validation;
- state transitions;
- idempotency conflict behavior;
- verifier digest construction;
- signed-request verification;
- delivery policy validation;
- allowed-host matching.

### Integration Tests

- static config adapter;
- in-memory adapter;
- OpenBao adapter;
- create saga;
- rotation saga;
- cleanup worker;
- runtime projection refresh;
- gateway cache invalidation.

### Failure Injection Tests

Cover:

```text
DB intent write fails
process crashes after DB intent
OpenBao write fails
OpenBao write outcome unknown
OpenBao write succeeds but DB activation fails
DB activation succeeds but outbox publish fails
gateway misses event
cleanup worker fails
provider revocation fails
client retries create repeatedly
client reuses idempotency key with different request
client loses one-time API key response
```

### Security Tests

- no plaintext in API responses after creation;
- no plaintext in logs;
- no plaintext in traces;
- no plaintext in metrics;
- no plaintext in outbox;
- denied host cannot be bypassed;
- unauthorized service identity cannot resolve credentials;
- revoked credential fails closed;
- signed request replay is rejected.

### Load Tests

- gateway cache hit rate;
- gateway cache miss pressure on OpenBao;
- rotation invalidation latency;
- revocation latency;
- idempotency ledger contention;
- OpenBao read/write throughput.

## Milestone Order

### Milestone 1: Internal Contract

- Value objects.
- `SecretStore` trait.
- Static config adapter.
- In-memory adapter.
- Current gateway behavior preserved.

Exit criteria:

- existing gateway tests pass;
- source config hydration uses the resolver boundary;
- no OpenBao dependency yet.

### Milestone 2: Credential Metadata And Operations

- DB metadata model.
- operation records;
- idempotency records;
- credential state machine;
- management API skeleton.

Exit criteria:

- create/rotate/revoke commands are metadata-only;
- retries converge on one operation;
- pending-operation caps exist.

### Milestone 3: OpenBao Write Path

- OpenBao adapter.
- create saga.
- rotation saga.
- reconciliation for create failures.

Exit criteria:

- raw provider credentials are stored in OpenBao;
- DB only stores refs and metadata;
- failure injection covers every create midway point.

### Milestone 4: Runtime Delivery

- runtime projection;
- gateway cache;
- cache invalidation;
- revocation semantics.

Exit criteria:

- gateway resolves active refs only;
- revoked credentials fail closed;
- gateway does not call OpenBao on every request.

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
  -> application database for metadata
  -> OpenBao for raw provider credential material

API key model
  -> bearer keys for normal developer automation
  -> signed keys for root/management/high-risk automation

signed key default
  -> Coinbase-style JWT request token
  -> client-generated key pair preferred
  -> ES256 acceptable first default if Coinbase compatibility matters

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

- Which owner boundary is canonical: organization, workspace, project, or
  tenant?
- Which database owns credential metadata?
- What is the first OpenBao mount and path convention?
- Which OpenBao auth method should each service use?
- What revocation latency is required?
- Which credential kinds ship in the first UI?
- Which providers get first-class validation?
- Which signed-key algorithm ships first?
- Whether one-time display escrow is allowed at all.
- What default idempotency TTL should be.
- What default pending credential TTL should be.
- What support roles can see during incidents.

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
