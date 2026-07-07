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

### Current Implementation Slice

`trogon-gateway` now has a first scheduler-style credential lifecycle decider
under `secret_store::credential_lifecycle`.

This slice covers metadata-only lifecycle events:

```text
WriteRequested
WriteFailed
Activated
RotationRequested
RotationFailed
Rotated
Revoked
```

The decider intentionally does not write OpenBao and does not carry plaintext.
OpenBao writes remain a trusted command-handler or saga step. The decider records
the metadata facts before and after those side effects, using `CredentialRef`,
`CredentialMetadata`, status, owner, source, and kind.

The slice also implements a runtime event codec for the lifecycle event set and
proves the command path through `trogon-decider-runtime::CommandExecution` with
an in-memory stream store test. That test verifies:

```text
RequestCredentialWrite
  -> NoStream append precondition

ActivateCredentialWrite
  -> replay lifecycle events
  -> At(position) append precondition
```

The write path now has a first command-handler layer under
`secret_store::credential_lifecycle_handler`. It performs the immediate saga for
new credential material:

```text
PutCredential
  -> append WriteRequested
  -> write secret material to SecretStore
  -> read store metadata
  -> append Activated
```

If the `SecretStore` write fails, the handler appends `WriteFailed` with a
bounded failure reason and still keeps the raw submitted secret out of the event
stream. If the activation append fails after the secret-store write succeeds,
a retry can observe the existing `PendingWrite` state, repeat the side effect
with the caller-resubmitted plaintext, and append `Activated`. The handler also
has a recovery command that reads existing store metadata and appends the
missing `Activated` event without needing the original plaintext again.

The same handler also covers rotation and revoke operations:

```text
RotateCredential
  -> append RotationRequested
  -> rotate secret material in SecretStore
  -> read rotated metadata
  -> append Rotated

RevokeStoredCredential
  -> revoke secret material in SecretStore
  -> append Revoked
```

If the rotation side effect fails, the decider records `RotationFailed` and
returns the lifecycle state to the active credential so the rotation can be
retried. Revoke writes the backend first because an event that says revoked
while the secret is still usable would be the more dangerous failure. A revoke
append failure can be retried because the OpenBao-style revoke operation is
idempotent for an existing logical credential.
If the rotation side effect succeeds but the `Rotated` append fails, the handler
can recover by reading the rotated credential metadata and appending the missing
`Rotated` event from the current `RotationPending` state.
The handler also exposes a recovery planner that converts a replayed
`PendingWrite` or `RotationPending` state into the appropriate recovery command.
For pending writes, it reconstructs the expected `CredentialRef` from the
deterministic OpenBao credential id, parsing from the right side of the id so
owner ids that contain colons remain valid.

Successful handler operations now return a `CredentialLifecycleHandlerOutcome`
with both the lifecycle state and the appended stream position. That gives the
future management API and workers the same position-aware boundary used by the
scheduler, without forcing them to infer event positions from side effects.
`CredentialLifecycleRuntimeHandler` composes that lifecycle handler with
`RuntimeCredentialRegistry`: after a successful command append, it applies the
returned lifecycle state to runtime projections at the returned stream position.
This is the intended boundary for the credential management API because it keeps
the event-sourced write path and runtime state update in one application
service.

The gateway also provisions the NATS event stream, subject resolver, event
store, snapshot KV bucket, runtime projection checkpoint KV bucket, and recovery
worker checkpoint KV bucket needed by the durable lifecycle store:

```text
GATEWAY_CREDENTIAL_LIFECYCLE_EVENTS
  -> gateway.credentials.lifecycle.events.v1.>
  -> allow_atomic_publish = true

GATEWAY_CREDENTIAL_LIFECYCLE_SNAPSHOTS
  -> history = 1
  -> max_age = 0

GATEWAY_CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINTS
  -> history = 1
  -> max_age = 0

GATEWAY_CREDENTIAL_LIFECYCLE_WORKER_CHECKPOINTS
  -> history = 1
  -> max_age = 0
```

Gateway credential contracts now use Protocol Buffers rather than ad hoc JSON
for durable payloads owned by Trogonai. The wire contract lives under
`proto/trogonai/gateway/credentials/v1` and is exposed through `trogonai-proto`
as `trogonai.gateway.credentials.v1`.

The current proto package contains:

- `commands.proto` for credential management command request and response
  payloads;
- `events.proto` for credential lifecycle event payloads;
- `idempotency.proto` for management idempotency KV records;
- `projection.proto` for runtime projection checkpoint records;
- `state.proto` for metadata-only credential lifecycle state snapshots;
- `types.proto` for shared credential refs, statuses, sources, credential
  kinds, and storage backends;
- `worker.proto` for recovery worker checkpoint records.

The Rust decider still uses rich domain value objects; protobuf is the durable
contract at the event-store, command, and KV payload boundaries. The existing
HTTP JSON routes remain an adapter over that domain contract so current route
behavior does not change. Lifecycle event payloads preserve the credential
scope key, so integration credentials like
`openbao:tenant-1:github/primary:webhook_secret` can round-trip through the event
log without flattening to source scope. OpenBao-generated credential ids are now
owner-scoped because lifecycle streams are keyed by credential id. That prevents
two owners with the same source integration id from competing on one event
stream. OpenBao rotation now preserves that same scope when it returns the next
credential version, so rotated integration credentials still project back to the
same runtime integration.
Credential lifecycle commands now use the same scheduler-style snapshot boundary
as the scheduler deciders: command execution reads lifecycle snapshots before
replay and writes metadata-only `CredentialLifecycleStateSnapshot` payloads on a
fixed 32-event frequency. The snapshot payload includes lifecycle refs,
fingerprints, metadata, failure reasons, and revocation facts, but never
plaintext credential material.
`runtime_projection` can now build an active runtime credential projection from
a replayed lifecycle state and use that projection to resolve material from the
secret store.

Runtime projection refresh can rebuild the in-memory projection repository from
persisted lifecycle stream events, and gateway startup now persists a projection
checkpoint in `GATEWAY_CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINTS`. The first
refresh scans from sequence 1. Later refreshes scan from the checkpoint cursor,
group decoded raw events by credential id, reload each changed aggregate from
the lifecycle event store, replay that aggregate through the decider evolve
function, and apply only changed projection states. The checkpoint advances only
after refresh succeeds. The projection repository and credential cache are now
carried together by `RuntimeCredentialRegistry`, which can create resolvers that
share the same cache cleared by lifecycle refresh. Active states upsert runtime
credentials, revoked credentials remove only that credential kind, and affected
cache entries are invalidated. Runtime projection keys now support both
integration scope, such as `github/primary`, and source scope, such as
`discord`.

GitHub, GitLab, incident.io, Linear, Microsoft Graph, Sentry, Slack, and
Telegram now have
runtime-backed webhook route variants. The existing static-config routers remain
available for current deployments, but `github::runtime_router` verifies the
webhook signature by resolving `CredentialKind::WebhookSecret` from
`RuntimeCredentialRegistry`, `gitlab::runtime_router` verifies the GitLab
standard webhook signature by resolving `CredentialKind::SigningToken`,
`incidentio::runtime_router` verifies the incident.io standard webhook
signature by resolving `CredentialKind::SigningSecret`,
`linear::runtime_router` verifies the Linear webhook signature by resolving
`CredentialKind::WebhookSecret`, `microsoft_graph::runtime_router` verifies
Microsoft Graph notification `clientState` by resolving
`CredentialKind::ClientState`,
`sentry::runtime_router` verifies the Sentry webhook signature by resolving
`CredentialKind::ClientSecret`, `slack::runtime_router` verifies the Slack
request signature by resolving `CredentialKind::SigningSecret`, and
`telegram::runtime_router` verifies the Telegram webhook secret token by
resolving `CredentialKind::WebhookSecret`, `twitter::runtime_router` signs CRC
responses and verifies webhook signatures by resolving
`CredentialKind::ConsumerSecret`, and `notion::runtime_router` verifies webhook
signatures by resolving `CredentialKind::VerificationToken` from that registry.
The Discord gateway runner can resolve a source-scoped
`CredentialKind::BotToken` from the same registry before opening the WebSocket
connection.
`source_plugin` and `http` expose a mounting path that can swap GitHub, GitLab,
incident.io, Linear, Microsoft Graph, Notion, Sentry, Slack, Telegram, and
Twitter/X independently to
runtime-backed routers while leaving the other sources on their current
static-config paths. Gateway startup
can mount these routes with `TROGON_GATEWAY_ENABLE_RUNTIME_GITHUB_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_GITLAB_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_LINEAR_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_NOTION_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_SENTRY_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_SLACK_CREDENTIALS`,
`TROGON_GATEWAY_ENABLE_RUNTIME_TELEGRAM_CREDENTIALS`, and
`TROGON_GATEWAY_ENABLE_RUNTIME_TWITTER_CREDENTIALS`; these modes require
`OPENBAO_ADDR` and `OPENBAO_TOKEN` so the resolver can read active credential
refs from OpenBao on cache miss.
Discord gateway runtime mode is enabled with
`TROGON_GATEWAY_ENABLE_RUNTIME_DISCORD_CREDENTIALS` and uses the same OpenBao
environment.

The gateway now has a first internal credential management command API under
`/-/credentials`. It is disabled unless the operator sets
`TROGON_GATEWAY_CREDENTIAL_MANAGEMENT_ADMIN_TOKEN`; when enabled it also
requires `OPENBAO_ADDR` and `OPENBAO_TOKEN`. The API is intentionally small and
currently supports Discord bot tokens, GitHub webhook secrets, incident.io
signing secrets, Linear webhook secrets, GitLab signing tokens, Microsoft Graph
client states, Notion verification tokens, Sentry client secrets, Slack signing
secrets, Telegram webhook secrets, and Twitter/X consumer secrets:

```text
PUT /-/credentials/discord/bot-token
  -> PutCredential

POST /-/credentials/discord/bot-token/rotations
  -> RotateCredential

DELETE /-/credentials/discord/bot-token
  -> RevokeStoredCredential

PUT /-/credentials/github/{integration_id}/webhook-secret
  -> PutCredential

POST /-/credentials/github/{integration_id}/webhook-secret/rotations
  -> RotateCredential

DELETE /-/credentials/github/{integration_id}/webhook-secret
  -> RevokeStoredCredential

PUT /-/credentials/gitlab/{integration_id}/signing-token
  -> PutCredential

POST /-/credentials/gitlab/{integration_id}/signing-token/rotations
  -> RotateCredential

DELETE /-/credentials/gitlab/{integration_id}/signing-token
  -> RevokeStoredCredential

PUT /-/credentials/incidentio/{integration_id}/signing-secret
  -> PutCredential

POST /-/credentials/incidentio/{integration_id}/signing-secret/rotations
  -> RotateCredential

DELETE /-/credentials/incidentio/{integration_id}/signing-secret
  -> RevokeStoredCredential

PUT /-/credentials/linear/{integration_id}/webhook-secret
  -> PutCredential

POST /-/credentials/linear/{integration_id}/webhook-secret/rotations
  -> RotateCredential

DELETE /-/credentials/linear/{integration_id}/webhook-secret
  -> RevokeStoredCredential

PUT /-/credentials/microsoft-graph/{integration_id}/client-state
  -> PutCredential

POST /-/credentials/microsoft-graph/{integration_id}/client-state/rotations
  -> RotateCredential

DELETE /-/credentials/microsoft-graph/{integration_id}/client-state
  -> RevokeStoredCredential

PUT /-/credentials/notion/{integration_id}/verification-token
  -> PutCredential

POST /-/credentials/notion/{integration_id}/verification-token/rotations
  -> RotateCredential

DELETE /-/credentials/notion/{integration_id}/verification-token
  -> RevokeStoredCredential

PUT /-/credentials/sentry/{integration_id}/client-secret
  -> PutCredential

POST /-/credentials/sentry/{integration_id}/client-secret/rotations
  -> RotateCredential

DELETE /-/credentials/sentry/{integration_id}/client-secret
  -> RevokeStoredCredential

PUT /-/credentials/slack/{integration_id}/signing-secret
  -> PutCredential

POST /-/credentials/slack/{integration_id}/signing-secret/rotations
  -> RotateCredential

DELETE /-/credentials/slack/{integration_id}/signing-secret
  -> RevokeStoredCredential

PUT /-/credentials/telegram/{integration_id}/webhook-secret
  -> PutCredential

POST /-/credentials/telegram/{integration_id}/webhook-secret/rotations
  -> RotateCredential

DELETE /-/credentials/telegram/{integration_id}/webhook-secret
  -> RevokeStoredCredential

PUT /-/credentials/twitter/{integration_id}/consumer-secret
  -> PutCredential

POST /-/credentials/twitter/{integration_id}/consumer-secret/rotations
  -> RotateCredential

DELETE /-/credentials/twitter/{integration_id}/consumer-secret
  -> RevokeStoredCredential

GET /-/credentials/recovery/status
  -> read credential lifecycle recovery worker checkpoint status
```

Those routes translate HTTP input into credential value objects and call
`CredentialLifecycleRuntimeHandler`; they do not write OpenBao directly and they
do not append lifecycle events directly. Responses include credential refs,
lifecycle state, and stream position, but never echo plaintext secret material.
Successful commands update runtime projections immediately through the same
registry used by the runtime Discord gateway runner and the GitHub, GitLab,
incident.io, Linear, Microsoft Graph, Notion, Sentry, Slack, Telegram, and
Twitter/X webhook resolvers.
The recovery status route is read-only and returns only checkpoint metadata:
last scanned raw stream sequence, next scan sequence, consecutive failure count,
first failure time, retry-after time, retry-delay state, and stuck-recovery
state. It uses the same admin token boundary as the command routes and does not
return secret material, OpenBao paths, lifecycle event payloads, or idempotency
records.
The recovery worker also records OpenTelemetry counters under the
`trogon-gateway` meter:

```text
gateway.credential_lifecycle.recovery.passes
  outcome = idle | advanced | recovered | failed_recovery | retry_delayed | stuck | error

gateway.credential_lifecycle.recovery.errors
  reason = worker_error

gateway.credential_lifecycle.recovery.scanned_events

gateway.credential_lifecycle.recovery.recoveries
  status = planned | recovered | failed
  kind = all | write | rotation

gateway.credential_lifecycle.recovery.stuck_reports
```

The first alertable conditions should be:

```text
stuck_reports increases
  -> page or route to the operator owning OpenBao and lifecycle recovery

failed_recovery passes continue increasing while checkpoint_advanced_to stays absent
  -> investigate activation append failures or OpenBao metadata reads

retry_delayed remains true longer than the stuck-after policy
  -> use /-/credentials/recovery/status to confirm failure age and retry window
```

The management API now requires `Idempotency-Key` for create, rotate, and
revoke commands. The key is scoped by owner, command namespace, and target
credential id. The gateway computes an HMAC request fingerprint while the secret
is still in memory, stores only metadata-only response snapshots, replays the
same snapshot for an identical retry, and returns an idempotency conflict when
the same scoped key is reused with a different command fingerprint. The runtime
implementation persists those records as protobuf-encoded values in a NATS KV
bucket so retries converge across gateway restarts and replicas:

```text
GATEWAY_CREDENTIAL_MANAGEMENT_IDEMPOTENCY
  -> history = 1
  -> max_age = 24h
```

If a command fails before completion, the durable NATS KV ledger deletes the
in-progress record by revision so the same logical retry can execute again.
Completed records are kept until TTL and replay the original metadata-only
response. This prevents a transient handler failure from pinning a user action
in `idempotency request is already in progress` until the bucket TTL expires.

This keeps client retries from appending duplicate lifecycle commands while
preserving the plaintext boundary.

The credential lifecycle recovery worker now persists a durable NATS KV cursor
before scanning the lifecycle event stream from the next raw stream sequence.
The checkpoint also carries `consecutive_failure_count`,
`first_failure_unix_seconds`, and `retry_after_unix_seconds`, so failure pacing
survives gateway restarts and multiple replicas. Each scan groups decoded events
by the credential id carried in the event payload, reloads each changed
aggregate from the lifecycle event store, replays it into decider state, and
invokes the existing write or rotation activation recovery command for pending
states. This matters because a raw JetStream scan sees the hashed NATS subject,
not the domain stream id. The checkpoint only advances after planned recoveries
finish without failures. Failed activation keeps the raw stream cursor pinned,
saves bounded retry/backoff state, and reports a stuck recovery after the
failure age crosses the worker policy threshold. That retries without requiring
plaintext from the DB, outbox, or client and avoids hammering OpenBao every
worker interval. The worker is spawned with the credential management OpenBao
path and updates the same runtime projection registry used by gateway
credential resolution.

The runtime credential projection now has its own continuous checkpointed
refresh worker. Gateway startup still refreshes the in-memory projection before
mounting runtime credential resolvers, then the worker keeps scanning from the
same `GATEWAY_CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINTS` cursor so later
lifecycle events from this gateway, another replica, or a provider callback can
be applied without restarting the process. The worker advances the cursor only
after the projection refresh succeeds.

The operator enablement flow is now concrete:

```text
start NATS and OpenBao
  -> set OPENBAO_ADDR and OPENBAO_TOKEN
  -> set TROGON_GATEWAY_CREDENTIAL_MANAGEMENT_ADMIN_TOKEN
  -> enable the source runtime flag for any source that should resolve from OpenBao
  -> create, rotate, or revoke with /-/credentials plus Idempotency-Key
  -> gateway writes lifecycle events through CredentialLifecycleRuntimeHandler
  -> handler applies the returned decider state to RuntimeCredentialRegistry
  -> runtime source resolver reads active CredentialRef from OpenBao on cache miss
  -> recovery worker and projection worker keep later replicas convergent
```

Local compose documentation in
`devops/docker/compose/services/trogon-gateway/README.md` now shows the
copy-in and copy-out OpenBao smoke test, the Testcontainers adapter test, the
Testcontainers lifecycle-runtime E2E test, and example management API commands
with `x-trogon-admin-token` plus scoped `Idempotency-Key` headers.
The compose stack was also verified with `nats://nats:4222`, local OpenBao,
the gateway management API, and a real GitHub webhook-secret flow:

```text
PUT /-/credentials/github/compose-smoke-2/webhook-secret
  -> active CredentialRef v1
  -> OpenBao CLI reads copy-this-value-in-and-out

POST /-/credentials/github/compose-smoke-2/webhook-secret/rotations
  -> active CredentialRef v2
  -> OpenBao CLI reads rotated-copy-this-value-in-and-out at version 2

DELETE /-/credentials/github/compose-smoke-2/webhook-secret
  -> revoked CredentialRef v2
  -> OpenBao version 2 returns null data with deletion_time set
```

The real OpenBao lifecycle E2E is:

```text
mise exec -- cargo test -p trogon-gateway runtime_handler_with_openbao_testcontainer_applies_lifecycle_and_resolves_precise_value
```

It starts OpenBao with Testcontainers, writes `copy-this-value-in-and-out`
through `CredentialLifecycleRuntimeHandler`, resolves the value through
`RuntimeCredentialRegistry`, rotates to
`rotated-copy-this-value-in-and-out`, revokes the credential, and verifies that
the lifecycle event payloads do not contain either plaintext value.

The remaining event-sourcing work is production orchestration:

```text
decide whether management idempotency records move from NATS KV to the future control-plane database
  -> tune lifecycle snapshot frequency and retention after production stream metrics exist
  -> write production OpenBao auth, HA, backup, cleanup, and alert runbooks
```

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
