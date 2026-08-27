# Credential Platform Spec

## Status

This is the working specification for the credential vault and API key
platform: the product model, the boundaries, the contracts, what exists in
code today, what is still open, and what is left to build. It replaces the
phased implementation plan the platform was originally scoped from. That
plan's phase numbering is gone; its remaining work is carried in
[Remaining Work](#remaining-work) and its open questions in
[Open Decisions](#open-decisions) below.

The goal is a Vercel-like credential management experience for Trogonai
while keeping raw credential material out of the normal application
database.

The specification covers six areas, each a section here:
CREDENTIAL_LIFECYCLE, API_CONTRACTS, AUTHORIZATION_MATRIX,
OPENBAO_OPERATIONS, RUNTIME_PROJECTION, UI_ACCEPTANCE. They live in one
file while the implementation is a single-crate prototype; they should
become separate files (`CREDENTIAL_LIFECYCLE.md` and peers) before the
implementation grows past it.

Every claim in this file about what "exists today" was checked directly
against the `trogon-gateway` source in this repository (grep and full file
reads of the relevant modules), not copied from `SECRET_STORE.md` or
`API_KEY.md`. Those two design documents predate the first implementation
slice and are the background reading, not the current state.

`API_KEY.md` has since been reconciled against ADR#0046, ADR#0048,
ADR#0050, ADR#0051, and ADR#0062 through ADR#0065, so it is now the design
of record for the API key platform rather than a superseded draft. It still
describes nothing that exists in code.

### Which document owns what

Every fact about this platform lives in exactly one of four places. When
two of them disagree, the one further up this list wins and the other is
the bug.

```text
docs/adr/*.md              WHY. Ratified decisions and their trade-offs.
                           Historical records: amended by a later ADR,
                           never edited to match a document below.

API_KEY.md                 WHAT the API key platform is. Data model,
                           key format, create and verification flows,
                           permissions, rate limits, rerolling,
                           idempotency, observability.

SECRET_STORE.md            WHAT the credential vault is.

CREDENTIAL_PLATFORM_SPEC   CONTRACTS and PLAN. Every endpoint, error
(this file)                code, idempotency namespace, the canonical
                           signed-request target, UI acceptance, build
                           order, testing, alerting, done criteria.
                           Points at the design documents rather than
                           restating them.
```

The endpoint table is here and not in `API_KEY.md` because a table split
across two files is a table that drifts. The data model is there and not
here for the same reason.

Open questions are called out inline as:

```text
DECISION NEEDED: <question>
recommendation: <recommendation>
```

No such question has been silently answered by writing this file; each one
still needs sign-off. Blocks marked `DECIDED (ADR#00NN):` were ratified and
link to the ADR that did it.

This file describes `trogon-gateway` as it exists, and that is deliberately
not the architecture of record.
[ADR#0023](./docs/adr/0023-secret-management-and-key-custody-direction.md)
places a platform secrets service in front of OpenBao and makes it the only
process holding an OpenBao client, which supersedes the gateway-embedded
shape described throughout this document. Read the OPENBAO_OPERATIONS and
RUNTIME_PROJECTION sections in particular as a record of prototype mechanics
that are scheduled to relocate, not as a description of where this
responsibility settles.
[ADR#0061](./docs/adr/0061-credential-platform-extraction-boundary.md)
carries the move list and the four decisions that block the extraction.

## Direction

Three credential families, three different homes:

```text
Provider credentials
  -> raw material Trogonai needs later on the customer's behalf
  -> OpenBao holds the material
  -> the application database holds metadata, refs, status, fingerprints,
     and delivery policy

Trogonai bearer API keys
  -> authenticate callers to Trogonai
  -> the raw key is shown once (ADR#0048)
  -> the application database holds a verifier digest and metadata

Trogonai signed API keys
  -> high-authority caller authentication
  -> the caller keeps the private key
  -> the application database holds public verification material and
     metadata
```

The enterprise-ready API key direction splits along the same line:

```text
Unkey-style control plane
  -> keyspaces, identities, roles, permissions, rate limits, rerolling,
     revocation, audit, analytics

Coinbase-style authentication proof
  -> asymmetric key pair, short-lived signed request token,
     method/host/path binding, nonce, expiry, optional IP allowlist
```

OpenBao is the internal credential backend. It is not the public product
API, and nothing about it reaches a customer-visible contract.

## Non-Goals For The First Version

- Do not build a general-purpose OAuth authorization server first.
- Do not require FAPI certification first.
- Do not require mutual TLS for every customer first.
- Do not store customer caller private keys in OpenBao by default.
- Do not expose OpenBao paths, mounts, tokens, or policies through the
  public API.
- Do not put raw provider secrets in the application database.
- Do not make every runtime request call OpenBao.
- Do not make Valkey the source of truth for secrets.

## Product Model

The user-facing model is deliberately small:

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

The UI never exposes saga, OpenBao, mount, path, or policy language. Users
see product states only; they are listed under UI_ACCEPTANCE below.

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

OpenBao is read:

- on credential creation or rotation;
- on runtime cache miss;
- on gateway or session startup and reconnect;
- during explicit refresh;
- during cleanup or reconciliation.

OpenBao is not read for every webhook or every ordinary runtime request.

### Gateway And Runtime Services

The gateway receives metadata projections and resolves only active
`CredentialRef` values when authorized.

Under [ADR#0023](./docs/adr/0023-secret-management-and-key-custody-direction.md)
that resolution goes through the platform secrets service, not through an
OpenBao client the gateway holds itself. The gateway is a consumer of refs,
and after the extraction it has no OpenBao address, token, or policy of its
own. The current `OpenBaoSecretStore` in `trogon-gateway` is prototype
scaffolding against that target, tracked in
[ADR#0061](./docs/adr/0061-credential-platform-extraction-boundary.md).

Runtime services receive typed values:

```text
GitHubWebhookSecret
SlackSigningSecret
DiscordBotToken
ProviderApiKey
OAuthRefreshToken
```

They never receive arbitrary OpenBao paths from callers.

## Implementation Defaults

Unless a later decision overrides them:

```text
credential backend
  -> event stream as the metadata source of truth (ADR#0047)
  -> OpenBao for raw provider credential material

API key model
  -> signed keys strongly recommended for all callers (ADR#0050)
  -> bearer keys as the policy-bounded compatibility tier
  -> root/management keyspaces are signed-only
  -> two keyspaces per project from creation: management, default (ADR#0063)
  -> permissions are the only authorization vocabulary; no scopes (ADR#0064)
  -> built-in roles only in the first milestone (ADR#0064)

signed key default
  -> Coinbase-style JWT request token, single-use and fully bound (ADR#0051)
  -> client-generated key pair only; the platform never holds private keys
  -> Ed25519 default, ES256 accepted for compatibility (ADR#0050)
  -> 1 minute token lifetime, 2 minute ceiling, 30s skew tolerance
  -> replay store in NATS KV, keyed by (key_id, jti), always fail closed

bearer key default
  -> tg_<env>_<key_id>_<secret><checksum> (ADR#0062)
  -> HMAC-SHA-256 verifier under a versioned pepper, constant-time compare
  -> pepper in OpenBao KV, resolved at boot, never on the request path
  -> 24h reroll grace; 1h in management keyspaces (ADR#0065)

rate limits
  -> per-key and per-project only in the first milestone (ADR#0064)
  -> counters in NATS KV; fail open on product traffic, closed on management

idempotency
  -> scoped by owner/workspace and command namespace
  -> target resource included for targeted operations
  -> metadata-only replay snapshots

runtime
  -> projection plus cache
  -> OpenBao on cache miss or refresh, not on every request

cleanup
  -> logical cleanup first
  -> physical cleanup async and idempotent
```

## Credential Lifecycle (CREDENTIAL_LIFECYCLE)

There are three separate lifecycle layers in play: the event-sourced decider
snapshot (implemented), the per-version CredentialStatus used by the
SecretStore adapters (implemented, and distinct from the decider), and the
broader CredentialState/IntegrationState/OperationState machines specified
below (not built at all). They are easy to conflate because they
share vocabulary (active, revoked, failed); this section keeps them apart on
purpose.

### Layer 1: the decider snapshot (implemented)

Source: `proto/trogonai/gateway/credentials/state/v1/state.proto` and
`credential/commands/state.rs` (`initial_state()`, `evolve()`). There is one
snapshot per credential_id, with exactly six cases:

```text
missing            (CredentialMissingState, the initial state)
pending_write      (PendingCredentialWriteState)
active             (ActiveCredentialState: metadata + previous_versions[])
write_failed       (FailedCredentialWriteState)
rotation_pending   (RotationPendingCredentialState, wraps an active state)
revoked            (RevokedCredentialState)
```

All transitions below are implemented today, verified by reading `evolve()`
in full:

```text
missing --WriteRequested--> pending_write
  otherwise: WriteRequestedAfterStart

pending_write --WriteFailed--> write_failed
  otherwise: WriteFailedWithoutPendingWrite

pending_write --Activated--> active
  otherwise: ActivatedWithoutPendingWrite
  (the Activated event's own metadata must carry CredentialStatus::Active,
  otherwise: MetadataNotActive)

active --RotationRequested--> rotation_pending
  otherwise: RotationRequestedWithoutActiveCredential

rotation_pending --RotationFailed--> active
  (reverts to the pre-rotation active state, so a new rotation can be tried)
  otherwise: RotationFailedWithoutPendingRotation

rotation_pending --Rotated--> active
  (the new version becomes active; the prior ref moves into
  previous_versions)
  otherwise: RotatedWithoutPendingRotation

active --Revoked--> revoked
  otherwise: RevokedWithoutActiveCredential
```

Two additional error variants guard cross-cutting invariants rather than a
single transition: `CredentialRefMismatch` (an event's ref does not match
the aggregate it is being applied to) and `RotationVersionNotNewer` (a
Rotated event whose version is not strictly newer than the previous one).

There is a real asymmetry worth flagging. `write_failed` is a terminal dead
end in the decider: no arm of `evolve()` accepts any event while the current
state is `write_failed`, since only `missing` accepts `WriteRequested`. The
handler layer (`CredentialHandler::put` in `handler.rs`) can only route
around this before a WriteFailed event is actually appended: if the append
of an Activated event fails after the secret write already succeeded, a
second `put()` call detects the still-pending write through
`ensure_write_requested` and resumes from there. Once `record_write_failure`
has appended a WriteFailed event, that credential_id has no decider path
forward at all. By contrast, `rotation_pending --RotationFailed--> active`
is explicitly a recovery transition; rotation failures are not terminal.
This gap is exactly what the planned `resubmission_required` state is meant
to close, and that state does not exist in code today.

`revoked` is also terminal: there is no transition out of it in `evolve()`.
There is no `destroyed` or `expired` case in the snapshot at all; those
concerns live one layer down, in CredentialStatus.

### Layer 2: CredentialStatus (per-version, implemented, not the decider)

Source: `credential/commands/domain/credential_status.rs` and every
SecretStore adapter (`openbao_secret_store.rs`, `in_memory_secret_store.rs`,
`static_config_secret_store.rs`, `mock_openbao_secret_store.rs`).

```text
pub enum CredentialStatus { Pending, Active, Previous, Revoked, Expired, Destroyed }
```

This describes one stored version's status as computed by the store
adapter; it is not the decider's aggregate state, and it is easy to
mistake one for the other since both use words like "active" and
"revoked". Grepping `CredentialStatus::` across every adapter shows only
four of the six declared variants are ever actually constructed:

```text
Active    -> assigned on write and on successful rotation, all adapters
Previous  -> assigned to the prior version after a successful rotation
Revoked   -> assigned on revoke() (OpenBao: derived from a non-empty
             deletion_time on that KV v2 version)
Destroyed -> assigned on destroy() (OpenBao: derived from the KV v2
             version's destroyed flag)
Pending   -> never constructed by any adapter
Expired   -> never constructed by any adapter
```

`Pending` and `Expired` are declared enum variants with zero production
call sites today; they are reserved, not shipped. `is_readable()` is true
only for `Active` and `Previous`; `is_writable()` is true only for `Active`.

One OpenBao-specific detail worth carrying into OPENBAO_OPERATIONS:
`revoke()` soft-deletes every version from 1 through the current version,
not only the version passed in, so a version's `Revoked` status can be
produced by a revoke call targeting a different, later version. `destroy()`
only targets the exact version it is given.

DECISION NEEDED: should Pending and Expired remain reserved, unused
CredentialStatus values, or should the planned lifecycle layer wire them up (for example,
Expired for a TTL-driven per-version expiry) or remove them so the enum
only lists what the store layer actually produces?
recommendation: keep them reserved. The planned
CredentialVersionState additions (resubmission_required,
revocation_requested, destroy_requested, destroyed, cleanup_failed) already
plan a richer version-state set that will likely absorb or replace this
enum; removing the placeholders now and reintroducing similar ones during
that work is churn with no benefit in between.

### Layer 3: planned upper lifecycle states (not built)

None of the following exist as Rust types or proto messages anywhere in the
repository today; they are the planned upper lifecycle layer, written out
here so this file is a complete lifecycle picture rather than only the
implemented part:

```text
CredentialVersionState (additions to CredentialStatus above)
  resubmission_required
  revocation_requested
  destroy_requested
  destroyed
  cleanup_failed

CredentialState (a new state machine, not the decider snapshot above)
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

CredentialState, IntegrationState, and OperationState sit above the decider
and CredentialStatus; they are not replacements for either. The Product
Model above implies CredentialState is closer to what users actually see
(draft, saving, ready, failed, ...; see UI_ACCEPTANCE), while the decider
snapshot and CredentialStatus stay internal machinery.

DECISION NEEDED: does the decider's write_failed map onto CredentialState
failed as a recoverable state, or is the
resubmission_required transition needed in evolve() first?
recommendation: add the resubmission_required transition to the decider
before wiring CredentialState failed to any retry UI. Today, offering a
"retry" action against a write_failed credential would offer an action the
decider has no path to accept.

DECIDED (ADR#0046): the hierarchy is organization over project, and the
owner boundary is the project: an immutable project id. Resource names,
OpenBao paths, and event streams anchor at the project and never embed the
organization above it. See
docs/adr/0046-project-anchored-resource-hierarchy.md.

DECIDED (ADR#0047): the event stream is the source of truth for credential
metadata. Operational records live in NATS KV, listing surfaces arrive
later as projections over the same streams, and no relational store enters
the first version. See
docs/adr/0047-event-sourced-credential-metadata.md.

DECISION NEEDED (Decisions Still Needed): what default pending-credential
TTL should apply, separate from the idempotency TTL?
recommendation: define a pending-write TTL distinct from the idempotency
ledger's 24 hour window. The two are different concerns: idempotency TTL
is a dedup window, pending-credential TTL is an abandonment window. The
recovery worker already treats an activation stuck for 30 minutes
(`DEFAULT_STUCK_AFTER = Duration::from_secs(30 * 60)`, in
`recovery_worker.rs`) as actionable; align the new pending-credential TTL
with that existing number (or slightly above it) rather than inventing an
unrelated figure.

## API Contracts (API_CONTRACTS)

### Internal admin API (implemented today)

Mounted at `/-/credentials` (`main.rs`: `app.nest("/-/credentials",
credential_management_routes)`), gated by a single shared secret sent as
the `x-trogon-admin-token` header, compared in constant time
(`provided.as_bytes().ct_eq(admin_token.as_str().as_bytes())`). There is no
per-caller identity; the token is the only authorization boundary, and
owner_id is caller-supplied in the request body rather than derived from an
authenticated identity (see AUTHORIZATION_MATRIX for the consequences).

Eleven of the twelve declared `SourceKind` variants have routes wired
(discord, github, gitlab, incidentio, linear, microsoft-graph, notion,
sentry, slack, telegram, twitter); `Datadog` exists as a domain value with
no admin route at all. Each wired source exposes:

```text
PUT    <base>              create (or replay, if idempotent)
DELETE <base>               revoke
POST   <base>/rotations     rotate
```

Idempotency rule, identical in shape for create/rotate/revoke, all through
`CredentialCommandIdempotencyStore`:

```text
client sends an `idempotency-key` header
  missing -> MissingIdempotencyKey (400)

server derives IdempotencyScope { owner_id, command_namespace,
  target_resource_id (= credential_id), idempotency_key } and hashes the
  four fields (SHA-256) into the NATS KV storage key

command_namespace is one of:
  Create ("credential.create")
  Rotate ("credential.rotate")
  Revoke ("credential.revoke")

server also computes RequestFingerprint = HMAC-SHA256(admin_token, parts):
  create: [namespace, owner_id, scope_key, credential_id, kind, secret_value]
  rotate: [namespace, owner_id, scope_key, credential_id, version, kind,
           secret_value]
  revoke: [namespace, owner_id, scope_key, credential_id, version, kind]
           (no secret_value; revoke has none)

begin(scope, fingerprint) -> Execute | Replay(response) | Conflict | InProgress
  Execute:    proceed with the command
  Replay:     same key + same fingerprint as a completed call
              -> return the stored response verbatim, do not re-execute
  Conflict:   same key + different fingerprint -> 409
  InProgress: same key + same fingerprint, call still in flight -> 409

on success: complete(scope, response) persists the response for replays
on any failure after begin(): abandon(scope) clears the in-progress record
  so a genuinely new attempt is not permanently blocked
```

The ledger is a NATS JetStream KV bucket
(`GATEWAY_CREDENTIAL_MANAGEMENT_IDEMPOTENCY`, history=1, max_age=24h),
so the only expiry today is a blanket 24h bucket age, not a per-operation
TTL. The record shape it stores (fingerprint, status, response) is a subset
of the target contract below.

Plaintext: never returned by this API. `CredentialCommandResponse` and
`CredentialRefResponse` only ever carry id, version, owner_id, source,
scope_key, kind, status, decider state name, and stream_position; there is
no secret-bearing field anywhere in either struct or in the proto record
used to persist a replay.

Error mapping, verified against every `CredentialManagementHttpError`
variant and its `IntoResponse` impl:

```text
Unauthorized                 -> 401  "unauthorized"
MissingIdempotencyKey        -> 400  "missing idempotency key"
IdempotencyConflict          -> 409  "idempotency conflict"
IdempotencyInProgress        -> 409  "idempotency request is already in progress"
IdempotencyStore(_)          -> 500  "credential management idempotency failed"
RecoveryCheckpoint(_)        -> 500  "credential recovery checkpoint failed"
InvalidInput(_)              -> 400  "invalid credential management request"
CommandFailed(_)             -> 500  "credential management request failed"
UnexpectedCredentialState(_) -> 500  "credential management request failed"
InvalidState(_)              -> 500  "credential management request failed"
```

These are this API's own ad hoc strings, not the public error-code
vocabulary defined below (validation_failed, permission_denied, idempotency_conflict,
pending_operation_exists, credential_not_ready, secret_write_failed,
needs_secret_resubmission, host_not_allowed, runtime_service_not_allowed,
secret_store_unavailable, provider_registration_failed, cleanup_pending).
Only `idempotency_conflict` lines up by name; the rest have no equivalent
here because this internal API has no vault, operation, or resubmission
concept yet.

There is also `GET /-/credentials/recovery/status` (separately mounted
`recovery_status_router`, same admin token), returning
`CredentialRecoveryStatusResponse` (last_scanned_sequence,
next_scan_sequence, consecutive_failure_count, first_failure_unix_seconds,
retry_after_unix_seconds, retry_delayed, stuck_recovery). Read-only,
metadata-only, no idempotency key required.

DECISION NEEDED: should the internal /-/credentials admin API adopt the
public error-code vocabulary now, or keep its own strings until the public
API replaces it?
recommendation: keep the internal API's own codes. This is an internal
admin-token-gated per-source API, a bridge rather than the public surface;
adopting the public vocabulary now would imply guarantees (vault_id,
operation_id) that this API does not have.

### Idempotency contract (target shape, partially built)

The client supplies an opaque key. The server supplies the scope. The KV
ledger described above implements the fingerprint/status/response core of
this; the rest is unbuilt.

```text
IdempotencyRecord
  owner_id
  workspace_id       (project id per ADR#0046)
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

Uniqueness:

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

Acceptance:

- retrying create ten times creates one credential intent;
- retrying rotate ten times creates one pending version;
- the same idempotency key with a different body returns conflict;
- two owners can use the same raw idempotency key without collision;
- response snapshots never contain raw secrets or one-time key material.

### Public API (nothing built)

Every endpoint below is unbuilt. There is no vault, operation, or
resubmit-secret concept behind any of them today. Paths are parent-scoped
resource names rooted at the project, following ADR#0046 section 3.

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

POST   /v1/projects/{project}/api-keyspaces
GET    /v1/projects/{project}/api-keyspaces
GET    /v1/projects/{project}/api-keyspaces/{keyspace}
PATCH  /v1/projects/{project}/api-keyspaces/{keyspace}
DELETE /v1/projects/{project}/api-keyspaces/{keyspace}

POST   /v1/projects/{project}/api-keyspaces/{keyspace}/keys
GET    /v1/projects/{project}/api-keyspaces/{keyspace}/keys
GET    /v1/projects/{project}/api-keyspaces/{keyspace}/keys/{key}
PATCH  /v1/projects/{project}/api-keyspaces/{keyspace}/keys/{key}
POST   /v1/projects/{project}/api-keyspaces/{keyspace}/keys/{key}/reroll
POST   /v1/projects/{project}/api-keyspaces/{keyspace}/keys/{key}/revoke
POST   /v1/projects/{project}/api-keyspaces/{keyspace}/keys/{key}/public-keys
POST   /v1/projects/{project}/api-keyspaces/{keyspace}/keys/{key}/public-keys/{public_key}/revoke

GET    /v1/projects/{project}/operations/{operation_id}
```

The flat `/v1/api-keys` shape an earlier draft used is superseded by the
project-anchored form above (ADR#0046 section 3).

The `{project}` segment is an authorization statement, not just addressing.
Admission derives the expected project from the authenticated caller
context and rejects a request whose `{project}` does not match it.

There is deliberately no public `POST /v1/api-keys/verify`. Verification is
an admission-time internal operation, not a customer-callable route. Unkey
exposes one because verification-as-a-service is their product, and
Trogonai is verifying its own callers; exposing it would publish an
unauthenticated oracle for testing candidate keys.

Key create accepts a `kind`, and there is no server-generated private key
option under either:

```text
kind: signed
  -> register a client-generated public key (recommended; the only kind
     the management keyspace permits)

kind: bearer
  -> create a verifier-backed raw key shown once
  -> rejected where keyspace bearer_issuance is disallowed
```

The `management` and `default` keyspaces are created by project creation
(ADR#0063 section 3), so `POST .../api-keyspaces` is for additional ones
and neither of the two built-ins can be deleted while it holds active keys.

Idempotency rule per write command, following the idempotency contract
above:

```text
POST /v1/projects/{project}/credential-vaults
  namespace credential_vault.create, scoped by owner_id + key (no target
  yet since the vault_id does not exist before this call)

PATCH .../{vault_id}, POST .../archive, POST .../restore
  namespace credential_vault.update / .archive / .restore, targeted by
  vault_id

POST /v1/projects/{project}/credentials
  namespace credential.create, targeted by credential_id if the client can
  pre-assign one, otherwise scoped by owner_id + key alone

PATCH /v1/projects/{project}/credentials/{credential_id}
  namespace credential.update, targeted by credential_id

POST .../rotate
  namespace credential.rotate, targeted by credential_id (matches the
  internal API's existing rotate scoping)

POST .../resubmit-secret
  namespace credential.resubmit_secret, targeted by credential_id

POST .../revoke
  namespace credential.revoke, targeted by credential_id (matches the
  internal API)

POST .../archive, POST .../delete
  namespace credential.archive / .delete, targeted by credential_id

POST /v1/projects/{project}/api-keyspaces
  namespace api_keyspace.create, targeted by keyspace_id if the client can
  pre-assign one, otherwise scoped by project_id + name

PATCH .../api-keyspaces/{keyspace}, DELETE .../api-keyspaces/{keyspace}
  namespace api_keyspace.update / .delete, targeted by keyspace_id

POST .../api-keyspaces/{keyspace}/keys
  namespace api_key.create, targeted by key_id if the client can pre-assign
  one, otherwise scoped by keyspace_id + display_name

POST .../keys/{key}/reroll, POST .../keys/{key}/revoke
  namespace api_key.reroll / .revoke, targeted by key_id

POST .../keys/{key}/public-keys
  namespace api_key.add_public_key, targeted by public_key_fingerprint,
  which makes re-applying the same registration a no-op and is the same
  idempotency key ADR#0063 section 4 relies on for bootstrap

POST .../keys/{key}/public-keys/{public_key}/revoke
  namespace api_key.revoke_public_key, targeted by public_key_id

GET endpoints
  read-only, no idempotency key required
```

Responses are metadata-only by default:

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

Applied per endpoint: every endpoint above is metadata-only except the four
that legitimately hand back new secret material, and even those only
display it once, per
[ADR#0048](./docs/adr/0048-one-time-plaintext-exposure.md):

```text
POST /v1/projects/{project}/credentials
  -> returns the new secret value once, on the first non-replay execution
     only
POST .../rotate                -> returns the new secret value once, same
                                  one-time rule
POST .../api-keyspaces/{keyspace}/keys
  -> returns the bearer key once, same one-time rule, and only when the
     keyspace allows bearer issuance. Creating a signed key returns
     metadata only: the caller already holds the private key and the
     platform never had one
POST .../keys/{key}/reroll      -> returns the new bearer key once, same
                                  one-time rule. Signed keys rotate by
                                  public-key swap and mint nothing
POST .../resubmit-secret        -> receives plaintext in the request; the
                                  response should stay metadata-only (there
                                  is nothing new to display back)
POST .../keys/{key}/public-keys -> receives a public key in the request;
                                  the response is metadata only, and a
                                  private key is never accepted here
every other endpoint            -> never plaintext
```

The public error-code vocabulary is:

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
bearer_issuance_disallowed
algorithm_not_allowed
key_limit_reached
keyspace_in_use
```

The last four are API-key management failures that `validation_failed`
would flatten into an indistinguishable error. Each names a policy the
caller can act on: bearer issuance is off for this keyspace, the requested
signing algorithm is outside the keyspace's `allowed_algorithms`, the
keyspace is at `max_active_keys`, or the keyspace still holds active keys
and therefore cannot be deleted.

A delegation-ceiling violation is `permission_denied`, not a code of its
own (ADR#0064 section 5). The caller asked for authority they do not hold,
which is the same answer as any other authorization failure.

Mapped per endpoint by what can plausibly fail there:

```text
POST /v1/projects/{project}/credential-vaults
                                       validation_failed, permission_denied,
                                       idempotency_conflict
GET .../credential-vaults[/{id}]      validation_failed, permission_denied
PATCH .../{vault_id}                  validation_failed, permission_denied,
                                       idempotency_conflict
POST .../archive, .../restore          validation_failed, permission_denied,
                                       idempotency_conflict,
                                       pending_operation_exists

POST /v1/projects/{project}/credentials
                                       validation_failed, permission_denied,
                                       idempotency_conflict,
                                       pending_operation_exists,
                                       secret_write_failed,
                                       secret_store_unavailable,
                                       provider_registration_failed
GET .../credentials[/{id}]            validation_failed, permission_denied
PATCH .../{credential_id}             validation_failed, permission_denied,
                                       idempotency_conflict,
                                       credential_not_ready
POST .../rotate                       validation_failed, permission_denied,
                                       idempotency_conflict,
                                       pending_operation_exists,
                                       credential_not_ready,
                                       secret_write_failed,
                                       secret_store_unavailable
POST .../resubmit-secret              validation_failed, permission_denied,
                                       idempotency_conflict,
                                       needs_secret_resubmission (if OpenBao
                                       already has the value and no
                                       resubmission was actually needed),
                                       secret_write_failed,
                                       secret_store_unavailable
POST .../revoke                       validation_failed, permission_denied,
                                       idempotency_conflict,
                                       credential_not_ready, cleanup_pending
POST .../archive, .../delete          validation_failed, permission_denied,
                                       idempotency_conflict, cleanup_pending
GET /v1/projects/{project}/operations/{operation_id}
                                       validation_failed, permission_denied

POST /v1/projects/{project}/api-keyspaces
                                       validation_failed, permission_denied,
                                       idempotency_conflict
GET .../api-keyspaces[/{keyspace}]    validation_failed, permission_denied
PATCH .../api-keyspaces/{keyspace}    validation_failed, permission_denied,
                                       idempotency_conflict
DELETE .../api-keyspaces/{keyspace}   validation_failed, permission_denied,
                                       idempotency_conflict, keyspace_in_use
POST .../api-keyspaces/{keyspace}/keys
                                       validation_failed, permission_denied,
                                       idempotency_conflict,
                                       bearer_issuance_disallowed,
                                       algorithm_not_allowed,
                                       key_limit_reached
GET .../keys[/{key}]                  validation_failed, permission_denied
PATCH .../keys/{key}                  validation_failed, permission_denied,
                                       idempotency_conflict
POST .../keys/{key}/reroll            validation_failed, permission_denied,
                                       idempotency_conflict,
                                       bearer_issuance_disallowed
POST .../keys/{key}/revoke            validation_failed, permission_denied,
                                       idempotency_conflict
POST .../keys/{key}/public-keys       validation_failed, permission_denied,
                                       idempotency_conflict,
                                       algorithm_not_allowed,
                                       key_limit_reached (2 active public
                                       keys per signed key, ADR#0065)
POST .../public-keys/{public_key}/revoke
                                       validation_failed, permission_denied,
                                       idempotency_conflict
```

Two codes in that vocabulary, `host_not_allowed` and
`runtime_service_not_allowed`, describe runtime resolution failures rather
than management-API failures; they belong to the delivery-policy checks
described in RUNTIME_PROJECTION (allowed_hosts, allowed_runtime_services),
not to any of the endpoints above.

Caller-authentication failures are a different surface and use a different,
deliberately coarse vocabulary. They are returned by admission on whatever
route the caller was calling, not by a management endpoint:

```text
unauthenticated   every credential-shaped failure: unknown key id, revoked
                  or expired key, bad verifier, bad signature, expired or
                  future-dated token, spent jti, target mismatch, payload
                  digest mismatch. One code, because distinguishing them
                  hands an attacker the oracle the absent public verify
                  endpoint was withheld to avoid.
nonce_required    carries a fresh server nonce to bind into the retry. Not
                  a leak: it is a protocol step the client must complete,
                  and it is returned before any key material is judged.
rate_limited      carries retry-after.
```

Target mismatch is the one case that returns a detail alongside the coarse
code: the verifier's own computed canonical target, per the API-contract
rules above. Without it every SDK integration debugs blind.

The detail carries an ordering requirement, or it becomes the oracle it is
carved out of. It is emitted only after the signature has verified against
a registered, live public key, and never for an unknown, revoked, or
expired key. A caller who reaches that point holds the private key, so the
only thing the detail tells them is how the server serialized the request
they themselves sent. Every other failure returns the bare code.

DECIDED (ADR#0048): a replay of the same idempotency key returns metadata
only, never plaintext, even for the caller who originally received it on
the first, non-replay execution. See
docs/adr/0048-one-time-plaintext-exposure.md.

DECIDED (ADR#0048): no server-side escrow of one-time material exists in
any form. Recovery from a lost one-time response is reroll
(Trogonai-issued keys) or resubmission (provider-supplied secrets), never
recovery of the original value. See
docs/adr/0048-one-time-plaintext-exposure.md.

DECISION NEEDED (Decisions Still Needed): what default idempotency TTL
should apply on the public API?
recommendation: keep the 24 hour figure already implemented in the internal
API's KV ledger (`CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE = Duration::
from_secs(24 * 60 * 60)`) as the ratified default, rather than choosing a
new number for the public API alone; the two surfaces will likely share a
ledger once the persistence work lands.

DECISION NEEDED: which providers get first-class validation on rotate (the
rotation saga's "validate if the provider supports validation" step)?
recommendation: none today, and that should be stated plainly rather than
implied. `SecretVerifier` exists as a value-object wrapper
(`secret_store/secret_verifier.rs`) but it only wraps a string; there is no
per-provider validation call anywhere in the eleven wired sources today.
Prioritize adding first-class validation for the providers that already
have a working write/rotate/revoke path through the internal admin API
before adding it for anything not yet wired.

### Canonical signed-request target (nothing built)

[ADR#0051](./docs/adr/0051-fully-bound-request-signing.md) section 2 places
this definition here and requires it to land before signed-request
verification does. A target the client and the server
serialize differently is a signature failure neither side can debug, so the
rules below are the whole compatibility surface between every SDK and the
verifier.

The serialization is a single string, fields joined by one LF (`0x0A`),
with no trailing LF. None of the fields can contain an LF after the
normalization rules below, so the framing is unambiguous without length
prefixes.

```text
HTTP    "http/1" LF method LF authority LF path LF query
NATS    "nats/1" LF subject LF operation
```

The leading transport tag is what keeps a NATS subject from ever colliding
with an HTTP target that happens to normalize to the same bytes.

HTTP field rules:

```text
method     uppercase ASCII, verbatim. A request whose method is not already
           uppercase ASCII is rejected before verification, so there is no
           case-folding rule for the two sides to disagree about.

authority  host, lowercased; port appended only when it is not the default
           for the scheme. IDN hosts use their A-label (punycode) form.
           IPv6 literals are bracketed, lowercase hex, in the RFC 5952
           compressed form. Scheme is not a field because signed routes
           accept only https; cleartext is rejected at admission.

path       percent-encoding normalized: percent-escapes of unreserved
           characters (A-Z a-z 0-9 - . _ ~) are decoded, every remaining
           escape uses uppercase hex digits, and every other non-unreserved
           byte is encoded. `/` stays the segment separator. Otherwise the
           path is taken as sent: no dot-segment removal, no slash
           collapsing, no trailing-slash normalization, no case folding. A
           path containing a `.` or `..` segment is rejected rather than
           resolved. An empty path serializes as `/`.

query      empty when the request has no query string, so the field is
           present but empty and the LF framing stays fixed. Otherwise each
           parameter is percent-encoding normalized the same way as the
           path (with `&` and `=` encoded inside names and values), the
           encoded parameter strings are sorted by byte order, and they are
           rejoined with `&`. A parameter sent without `=` keeps its bare
           name, so `?a` and `?a=` do not collapse into one target.
```

Query is bound rather than dropped. Without it a token signed for
`GET /v1/.../credentials?page_size=25` would authorize the same call with
`page_size=100000`, which is the same substitution the payload digest
closes for bodies.

Sorting exists because HTTP clients and proxies reorder query parameters
freely, so an unsorted target would fail for reasons the caller did not
cause.

NATS field rules:

```text
subject    the concrete subject the message was published on, verbatim.
           Subjects are already a restricted alphabet, so no normalization
           applies. A subject containing `*` or `>` is rejected: a wildcard
           is not a target. Subject mapping that rewrites subjects is
           unsupported in front of signed subjects, per ADR#0051 section 6.

operation  the operation name from the payload envelope, verbatim.
```

The `tgt` claim carries the base64url SHA-256 (unpadded) of the canonical
target string, not the string itself. Constant width, no JSON escaping of
the LF framing, and nothing about it is secret: on a target mismatch the
verifier returns its own computed canonical target in the error detail, so
an SDK author can diff the two strings directly instead of comparing
hashes.

Worked examples:

```text
GET https://api.trogon.ai/v1/projects/p_9f2/credentials
    ?page_size=25&filter=kind%3Aslack

  http/1
  GET
  api.trogon.ai
  /v1/projects/p_9f2/credentials
  filter=kind%3Aslack&page_size=25

  tgt = m_hqRMLS9vuHqgfOxfDxNBpE3ybIjZ5monRX9nbloo0

NATS subject credential.command.rotate, operation RotateCredential

  nats/1
  credential.command.rotate
  RotateCredential

  tgt = OExB_oJqD9xvhssENaQeU-k1d5zufYFK5xAjN6CPZlc
```

The payload digest that rides alongside it:

```text
pdh = base64url(SHA-256(exact request body bytes)), unpadded

bodyless requests use the digest of the empty byte string:
  47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU
```

On NATS the digest covers the raw payload bytes with no canonicalization,
because a NATS payload is a single byte slice.

Two verifier obligations that are not serialization rules but fail the same
way when they are missed:

- the verifier rejects any authority outside its own configured set of
  served authorities before it compares targets, so a forged `Host` header
  cannot decide what a signature is checked against;
- an intermediary that rewrites paths, query strings, or bodies breaks
  signatures by design, which is the stated consequence in ADR#0051 rather
  than a bug to work around.

Test vectors ship as one shared fixture consumed by both the verifier tests
and the SDK test suites, so a divergence is a failing test rather than a
support ticket. The set covers, at minimum: mixed-case host, IDN host, IPv6
literal, default and non-default ports, over-encoded and under-encoded
paths, a path with reserved characters in a resource id, absent query,
single-parameter query, out-of-order multi-parameter query, repeated
parameter names, valueless parameters, empty body, and a NATS subject with
a multi-token name.

## Authorization Matrix (AUTHORIZATION_MATRIX)

The platform defines five roles, derived from the segregated store traits
so the split survives the extraction in
[ADR#0061](./docs/adr/0061-credential-platform-extraction-boundary.md):
`control_plane_write`, `gateway_read`, `lifecycle_worker_cleanup`,
`audit_read`, `break_glass_admin`. They exist as HCL under
`devops/openbao/policies/`, one file per role, with a `verify.sh` that
applies them to a throwaway dev OpenBao and asserts every allow and deny
outcome against real percent-encoded paths.

What does not exist is any binding between an identity and a policy. In
code there is exactly one OpenBao identity: a single static bearer token
(`OPENBAO_TOKEN`), sent unconditionally as `X-Vault-Token` by
`OpenBaoSecretStore::authorize()` for every operation, regardless of which
Rust component is calling (the admin API's command handler, the recovery
worker, the runtime projection resolver). There is no AppRole, no
Kubernetes auth, and no declarative apply (no Terraform), only the loop
documented in that directory's README. Until an auth method binds a service
to a role, the scoping in each policy file is a design, not a control, and
the gap table below is the accurate picture.

```text
control_plane_write
  planned:  put, rotate, metadata (the write path: create + rotate)
  today:    shares the single static token; the same code path also has
            revoke and destroy capability, which is broader than this role
            is meant to have
  gap:      no policy separates control_plane_write from revoke/destroy

gateway_read
  planned:  get, resolving only active refs the gateway is authorized for
  today:    the runtime resolver (RuntimeCredentialResolver::resolve) does
            gate on RuntimeIntegrationStatus::is_resolvable() before
            calling SecretStoreGet::get, but it authenticates with the same
            static token that can also put/rotate/revoke/destroy
  gap:      no distinct read-only OpenBao policy; the gateway process holds
            a token that can write

lifecycle_worker_cleanup
  planned:  revoke, destroy, scoped to owned paths
  today:    no lifecycle/cleanup worker exists at all. The recovery worker
            that does exist (recovery_worker.rs) only recovers stuck
            write/rotation activations via SecretStoreMetadata::metadata;
            it never calls revoke or destroy
  gap:      role and worker are both unbuilt

audit_read
  planned:  read metadata only, never raw secret values
  today:    no audit role, and no audit-fact concept, exists anywhere
  gap:      role unbuilt

break_glass_admin
  planned:  emergency read/write across scopes, audited
  today:    does not exist as a concept. The single static admin token is,
            functionally, a permanent unaudited break-glass credential: it
            can do every operation with no logging beyond ordinary tracing
  gap:      no break-glass concept, no audit trail specific to emergency
            access
  consumer: ADR#0063 section 6 gives this role its first concrete
            consumer, the operator CLI that registers an additional root
            public key when every root private key is lost. That path is
            unusable in production until an audit device is provisioned,
            so the audit-device gap is a blocker on it rather than
            adjacent work
```

The internal `/-/credentials` admin API has its own, separate
authorization boundary: one shared admin token, constant-time compared,
applied uniformly across create/rotate/revoke for all eleven wired sources
plus the recovery-status endpoint. Caller-supplied owner_id is trusted as
given; there is no separate per-owner authorization check layered on top.
The admin token is the entire authorization boundary for this API today.

DECISION NEEDED (Required Decision): first OpenBao auth method per
service.
recommendation: adopt OpenBao AppRole per service identity (one AppRole per
service, e.g. gateway, control-plane, lifecycle worker) rather than
Kubernetes auth or continuing with static tokens. AppRole works regardless
of whether the runtime is Kubernetes, and it maps directly onto the five
named policies. Keep the current static-token adapter for local dev only.

## OpenBao Operations (OPENBAO_OPERATIONS)

DECIDED (ADR#0052): production OpenBao must auto-unseal against a cloud KMS
key (GCP Cloud KMS or AWS KMS expected); the Shamir quorum seal is
prohibited in production except for deployments whose network cannot reach
a cloud KMS, and recovery keys exist for break-glass ceremonies only. See
docs/adr/0052-cloud-kms-production-seal.md.

### Path convention (verified in openbao_secret_store.rs)

```text
credential_path(owner_id, credential_id)
  = "trogonai/{owner_id}/credentials/{credential_id}"

credential_id (openbao_credential_id())
  = "openbao:{owner_id}:{scope_key}:{kind}"
  where scope_key is either the bare source name for a source-scoped
  credential (e.g. "discord"), or "{source}/{integration_id}" for an
  integration-scoped one (e.g. "github/primary")

full KV v2 HTTP path
  = "/v1/{mount}/{data|metadata|delete|destroy}/trogonai/{owner_id}/
     credentials/{credential_id}"
```

Worked example: owner_id "tenant-1", source github, integration "primary",
kind webhook_secret:

```text
credential_id = "openbao:tenant-1:github/primary:webhook_secret"
path          = "trogonai/tenant-1/credentials/openbao:tenant-1:github/
                 primary:webhook_secret"
```

Note owner_id appears twice in the full path, once as its own segment and
once embedded inside credential_id; `encode_path_segment` sanitizes each
occurrence independently, so this duplication is not a correctness problem,
just a naming quirk worth knowing about when reading raw OpenBao paths.

The mount defaults to `"secret"` (`OpenBaoMount::default()`), validated to
ascii alphanumeric plus `-` and `_` and `.`, non-empty.

### Custom metadata convention (write_custom_metadata(), verified)

Every write attaches exactly five path-level custom_metadata fields, all
string-typed:

```text
owner_id
credential_id
credential_kind
current_version   (stringified u64)
created_at         (RFC3339, Utc::now() at write time)
```

KV v2 custom_metadata is path-level, not per-version, so these five fields
describe the whole path's current state, not any one historical version.
Planned metadata fields that do not exist yet:

```text
workspace_id            (the project id per ADR#0046)
integration_id          (needs integration records)
operation_id            (needs operation records)
credential_version_id   (needs its own convention, since KV v2 metadata has
                        no native per-version attribution)
```

### Auth method (implemented today)

A single static bearer token, sent as `X-Vault-Token` unconditionally for
every operation (`authorize()` in `openbao_secret_store.rs`), loaded from
the `OPENBAO_TOKEN` environment variable. No AppRole, Kubernetes auth, or
certificate auth is configured anywhere in the code. A static dev token is
not a production answer; choosing the first auth method per service is an
open decision, listed below.

### Read patterns (implemented)

```text
put()      writes data, then writes custom metadata (two HTTP calls);
           wraps the value in {"data": {"value": ...}} on the KV v2 data
           endpoint

get()      reads the data endpoint at a specific version (?version=);
           always returns SecretMaterial::Plaintext(SecretString) in the
           OpenBao adapter today. SecretMaterial::Verifier exists as an
           enum variant, but the OpenBao adapter never constructs it

metadata() reads the metadata endpoint; derives per-version CredentialStatus
           from the KV v2 version list, see CREDENTIAL_LIFECYCLE Layer 2

rotate()   writes a new version via the data endpoint, then metadata

revoke()   calls the KV v2 soft-delete endpoint for every version from 1
           through the current version, not only the version passed in

destroy()  calls the KV v2 destroy endpoint for exactly the one target
           version passed in, driven by the destroy lifecycle saga
           (destroy_requested, destroyed, cleanup_failed) and the admin
           destroy route. Logs the reason via tracing::info! only; there is
           no audit-fact record
```

This lines up with the Architecture Boundaries list of when OpenBao is read
(create/rotate, runtime cache miss, startup/reconnect, explicit refresh,
cleanup/reconciliation): the implemented resolver reads on cache miss, and
the recovery worker reads metadata() to recover stuck activations. The
"during cleanup" pattern has no code yet since async cleanup itself is
unbuilt (see RUNTIME_PROJECTION and Remaining Work).

DECIDED (ADR#0046): `trogonai/{owner_id}/credentials/{credential_id}` and
its mount are ratified as-is, with owner_id understood as project id.
Names anchor at the project and never embed the organization above it. See
docs/adr/0046-project-anchored-resource-hierarchy.md.

DECISION NEEDED: should revoke() keep soft-deleting every version 1 through
current, or move to targeting only the version(s) a caller intended to
revoke?
recommendation: change revoke() to accept an explicit version (or version
range) once the revocation_requested state exists, instead of
implicitly reaching back to version 1. Blanket revocation of all history is
surprising behavior for an API that reads as "revoke this one ref," and it
makes reconciliation between OpenBao and the future DB harder to reason
about.

## Runtime Projection (RUNTIME_PROJECTION)

### Shape that exists today (runtime_projection.rs, verified)

```text
RuntimeIntegrationProjection
  key:          RuntimeIntegrationKey
  owner_id:     CredentialOwnerId
  status:       RuntimeIntegrationStatus
  version:      u64
  credentials:  BTreeMap<CredentialKind, CredentialRef>

RuntimeIntegrationKey
  source: SourceKind
  scope:  Source | Integration(String)
  (one key per source for source-scoped credentials, or per source +
  integration_id pair for integration-scoped ones; not one key per
  credential)

RuntimeIntegrationStatus
  Active | Disabled | Archived | Deleted | Pending | Failed
  only Active.is_resolvable() is true
```

Grepping every non-test call site that constructs `RuntimeIntegrationStatus`
(`from_credential_state()` at line 291 and `active_runtime_projection()` at
line 346, both above the `#[cfg(test)]` boundaries at lines 1401 and 1574)
shows
only `Active` is ever produced in production code.
`from_credential_state()` maps the decider's `Active` and `RotationPending`
cases to `RuntimeIntegrationStatus::Active`, and returns `None` (no
projection at all) for `Missing`, `PendingWrite`, `WriteFailed`, and
`Revoked`. `Disabled`, `Archived`, `Deleted`, `Pending`, and `Failed` are
declared but never constructed outside test helper code; they are
placeholders for the planned CredentialState/IntegrationState machinery,
not wired to anything today.

That matters for any claim that fail-closed behavior on revoked or disabled
credentials exists. The revoked half is real: a Revoked event removes the
credential from its
projection entirely (see Invalidation below), so the resolver simply has
nothing to resolve. The disabled half has no code behind it; there is no
runtime scenario today that produces a Disabled projection for the
resolver to fail closed against.

### Refresh paths (three, all implemented)

```text
refresh_from_credential_stream / _incremental
  full or incremental rebuild from raw JetStream events; replays each
  affected credential_id's whole stream through evolve()

refresh_from_credential_stream_checkpointed
  same as incremental, tracked in a NATS KV checkpoint bucket
  (GATEWAY_CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINTS, history=1,
  max_age=0) so the periodic worker resumes where it left off

apply_state
  synchronous path used by CredentialRuntimeHandler after every
  put/rotate/revoke; updates the in-memory projection immediately without
  waiting for the periodic worker
```

### Invalidation rules (verified precisely against the code)

```text
apply_state(), when from_credential_state() returns a projection
  (decider state is Active or RotationPending):
    merge the projection, then cache.clear() -- a full wipe of every
    cached credential across every integration, not only the one that
    changed

apply_state(), when the decider state is Revoked:
    remove_credential_ref() calls cache.invalidate(&credential_ref) for
    just that one credential, then removes it from the projections map

apply_state(), when the decider state is Missing / PendingWrite /
  WriteFailed:
    no cache change, no projection change (nothing was ever projected for
    these states either)

refresh_runtime_projections_from_credential_events() (the bulk/periodic
  refresh path):
    always cache.clear() plus projections.replace_all(), an unconditional
    full rebuild, regardless of what actually changed
```

So the only selective, single-key invalidation anywhere in the code is the
revoke path. Every other mutation, including an ordinary single-credential
rotation applied synchronously through apply_state, flushes the entire
runtime cache. This is a real, verified behavior, not a guess: `cache.
clear()` appears at the merge branch of `apply_state` (line ~851) and again
in the bulk refresh path (line ~994), while `cache.invalidate(&credential)`
appears only in `remove_credential_ref` (line ~864) and its
`apply_state_to_projection` counterpart (line ~1113).

### Cache TTL and jitter (RuntimeCredentialCachePolicy, exact values)

```text
default ttl    = Duration::from_secs(300)
default jitter = Duration::from_secs(30)
```

Jitter is deterministic per CredentialRef, not randomized: `key_jitter()`
hashes `credential.to_string()` with a `DefaultHasher`, takes `hash %
jitter_nanos`, and subtracts that from the ttl in `expiry_offset()`. The
same credential always gets the same effective expiry point under a given
policy; this is stable per-key skew to avoid a thundering herd of
simultaneous expiries, not run-to-run randomness.

### Delivery policy (implemented, not yet populated)

`RuntimeIntegrationProjection` carries a `RuntimeDeliveryPolicy` alongside
key, owner_id, status, version, and the per-kind CredentialRef map:

```text
RuntimeDeliveryPolicy
  allowed_hosts              AllowedHosts { Unrestricted | Only(AllowedHost) }
  allowed_runtime_services   AllowedRuntimeServices { Unrestricted | Only(RuntimeServiceId) }
  injection_locations        InjectionLocations (default empty = deny)
  cache_ttl_override         Option<Duration> (may only shorten)
```

`workspace_id` is deliberately not a field. ADR#0046 makes
`CredentialOwnerId` the project id, so the projection's existing `owner_id`
already carries it; adding a second field would reintroduce the
workspace/project split that ADR removed.

Enforcement lives on `RuntimeCredentialResolver::resolve_for`, which takes
a `RuntimeDeliveryRequest` and denies before the secret store is touched,
so a denied caller cannot warm the cache or distinguish a present
credential from an absent one. The existing `resolve` delegates with an
empty request, which the permissive default admits, so the twelve shipped
source paths are unchanged.
`RuntimeDeliveryPolicy::effective_cache_ttl` clamps an override to the
configured TTL, so a policy can only narrow the ADR#0049 staleness bound,
never widen it.

Nothing populates these fields yet. The management API and the credential
event stream have to carry them before a policy can be configured in
production. Until then the default is permissive on hosts and runtime
services, and the fail-closed behavior is exercised only by tests.

DECIDED (ADR#0049): target p99 revocation-to-invalidation latency at or
under 5 seconds under normal operation; page when p99 exceeds 10 seconds
sustained over 5 minutes. The existing 300s ttl / 30s jitter is ratified as
the working cache default. See docs/adr/0049-revocation-latency-target.md.

DECISION NEEDED: should cache invalidation stay whole-cache-clear on every
projection merge, or move to per-CredentialRef invalidation now, ahead of
the outbox exists?
recommendation: move to per-CredentialRef invalidation now. `apply_state`
already knows the specific CredentialRef inside the Active/RotationPending
state it just merged, so invalidating just that entry (the way revoke
already does) is a small change, and it avoids flushing unrelated
integrations' cached material on every unrelated rotation.

## UI Acceptance (UI_ACCEPTANCE)

Nothing described in this section exists in the UI or public API today. The
console app is a single-route scaffold with no data layer, and the
credentials proto packages define no RPC services a browser client could
call. Everything below is acceptance criteria for work that has not
started, written now so the public API and the UI build against agreed
behavior.

### Product states

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

These are the states of a credential the user is looking at, scoped to their
own action. They are not sourced from the incomplete provision read model,
which is operator-only and never rendered; see that section for the split
between phases that hide a credential entirely and phases that show one under
its own product status.

### Vault list columns

```text
name
environment
kind
status
fingerprint
last used
allowed hosts
actions: add, rotate, resubmit secret, revoke, archive, delete, view audit
```

### API key list columns

```text
display name
keyspace
kind                 bearer | signed
status               active | rotating | revoked | expired
public prefix        tg_<env>_<key_id>, bearer only, derived not stored
fingerprint          sha256:..., signed only
roles
direct permissions   distinct badge, never folded into the role display
expires at
last used
actions: create, reroll, revoke, add public key, revoke public key,
         view audit
```

`rotating` and the direct-permissions badge are not cosmetic choices:
ADR#0065 section 3 and ADR#0064 section 3 make them the product surface for
a half-finished rotation and for an out-of-role grant, both of which are
otherwise visible only in the data.

### Acceptance scenarios

Given a user with permission to add credentials to a vault
When the user enters credential metadata and a secret value, and the
  client generates an idempotency key before the first submit
Then the client submits exactly once, clears the plaintext secret from
  memory immediately after submit, and the UI shows an operation status the
  user can poll by operation id.

Given a user has already submitted a create request whose response has not
  arrived yet
When the same client, or a retry from a flaky connection, resubmits the
  identical request with the same idempotency key
Then the server treats it as a replay of the same operation, and the UI
  must not show a second pending credential.

Given a create request fails with secret_write_failed while its operation
  is still active
When the UI polls the operation
Then the UI offers a retry using the same idempotency key, rather than
  starting a new create.

Given a create request fails with secret_write_failed and the operation is
  no longer active
When the user returns to the credential
Then the UI shows resubmit secret instead of retry.

Given a credential enters needs_secret_resubmission
When the user opens the credential
Then the UI asks for the secret value again, through the same
  one-submit-and-clear-memory pattern as create, not a hidden retry.

Given a credential is cleanup_pending, logically revoked or disabled but
  not yet physically cleaned up
When the user views the vault list
Then the UI shows revoked or disabled there for runtime-safety reasons, and
  shows cleanup_pending detail only on the credential's own detail view.

Given a credential is mid-rotation (rotation_pending)
When runtime services resolve the credential during that window
Then the old active version keeps resolving until the new version reaches
  active, so rotation never breaks a currently running integration.

Given a bearer API key's raw value was shown once and the tab was closed
  before it was copied
When the user returns to the key's detail view
Then the UI states the key exists but its value cannot be recovered, and
  offers reroll as the only recovery action.

Given a bearer key was rerolled and its grace window has not elapsed
When the user views the key list
Then the old key shows as rotating with its expiry rather than as active,
  so a rotation somebody started and forgot to finish is visible in the
  product rather than only in the data.

Given a user is creating a signed key
When the UI walks them through it
Then it never offers to generate, upload, or accept a private key, and
  asks only for a public key produced by the CLI, because the platform
  never holds a private key (ADR#0050 section 2).

Given a keyspace whose policy disallows bearer issuance
When the user opens the create-key flow for it
Then bearer is not an offered option, and the flow explains the keyspace
  policy rather than failing with bearer_issuance_disallowed after submit.

Given a key carries direct permissions beyond its roles
When the user views the key list or its detail view
Then those permissions render as a distinct badge and the key is marked as
  carrying them, so an out-of-role grant is discoverable without reading
  the key's policy.

Given any credential state
When the user inspects it through the UI
Then the user never sees OpenBao paths, mounts, policies, or saga/outbox
  terminology, only the product states listed above.

Together these are the UI acceptance bar: the user
never sees OpenBao internals, never sees the raw secret after creation, the
UI cannot create duplicate pending records through retry, rotation never
breaks the old active credential before the new one is ready, and every
failed operation has a clear next action.

DECISION NEEDED (Required Decision / Decisions Still Needed): which
credential kinds ship in the first UI?
recommendation: ship the kinds already wired through the internal
/-/credentials admin API first (bot_token, webhook_secret, signing_token,
signing_secret, client_state, verification_token, client_secret,
consumer_secret, across discord, github, gitlab, incidentio, linear,
microsoft-graph, notion, sentry, slack, telegram, twitter), since those
already have a working write/rotate/revoke path end to end. Do not include
app_token or webhook_token in the first UI: neither is reachable through
any admin route today, and webhook_token specifically is not even
reachable through `CredentialKind::parse()`, which has no
`"webhook_token"` match arm despite the variant existing on the enum. That
parse gap should be fixed in code regardless of which kinds ship first.

## Lifecycle Sagas And Reconciliation

The gateway performs the immediate write, rotate, revoke, and destroy sagas
against OpenBao today, with a recovery worker for stuck activations. The
target sagas below add a database intent and an outbox, neither of which
exists. Revoke currently deletes from OpenBao synchronously in the same
request rather than deferring to async cleanup.

### Create saga

```text
1. Authorize command.
2. Create scoped idempotency record.
3. Create DB credential/version intent as pending_secret_write.
4. Write raw secret to OpenBao.
5. Mark version active.
6. Emit outbox event.
7. Gateway refreshes projection.
```

### Rotation saga

```text
1. Keep current active version active.
2. Create new pending version.
3. Write new secret to OpenBao.
4. Validate if the provider supports validation.
5. Promote new version to active.
6. Mark old version previous or revoked.
7. Emit outbox event.
```

### Cleanup rules

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

Runtime safety never depends on physical cleanup completing immediately.

### Reconciliation jobs

```text
DB -> Gateway
  -> implemented, as the checkpointed projection refresh worker

DB -> OpenBao
  -> not built: expected secret exists and has expected metadata

OpenBao -> DB
  -> not built: every managed secret has a DB owner or becomes orphan
     cleanup. Blocked on a list operation; the segregated store traits have
     no SecretStoreList, so the managed tree cannot be enumerated in code
     today (see devops/openbao/runbooks/orphan-openbao-secret-cleanup.md
     for the manual interim procedure)

Provider -> DB
  -> not built: provider-side revocation or disconnect reflected when
     possible
```

Acceptance for this area:

- every midway failure has a test;
- an OpenBao write success plus a DB activation failure can be reconciled;
- a DB pending with no OpenBao secret expires to resubmission;
- cleanup is idempotent.

### Incomplete provision read model (not built)

A create that appended `WriteRequested` and never reached `Activated` is
invisible. Nothing enumerates it, nothing counts it, and no operator surface
lists it. This section specifies the read model that closes that gap, and the
rule that it is never rendered to end users.

What is true today, verified in code:

```text
the runtime projection excludes incomplete provisions structurally
  -> RuntimeIntegrationProjection::from_credential_state()
     (runtime_projection.rs:291, arm at 308-317) returns Ok(None) for
     PendingWrite and WriteFailed, so no projection row is ever built

discovery is a raw stream scan, not a query
  -> recover_pending_credential_activations() (recovery_worker.rs:316)
     replays credential events from a checkpoint to find pending
     activations; there is no queryable set of them anywhere

the only operator surface reports worker health, not work
  -> GET /-/credentials/recovery/status returns
     CredentialRecoveryStatusResponse (credential_management.rs:1988):
     checkpoint sequences, consecutive_failure_count, retry_delayed,
     stuck_recovery. It answers "is the worker healthy", never "which
     credentials are stuck"
```

The consequence is that a credential can sit in `PendingWrite` or
`WriteFailed` indefinitely while every dashboard reads green, because the
recovery worker is scanning normally and simply has nothing it can do about
that credential. This is most acute for `WriteFailed`, which is terminal:
`WriteRequested` is accepted only from `Missing` (`state.rs:108-114`), so a
credential whose first OpenBao write failed has no path forward at all, and
today no surface says so.

#### Shape

```text
IncompleteProvision
  credential_id
  owner_id
  source
  kind
  phase            write_requested | write_failed | rotation_pending |
                   destroy_requested | cleanup_failed
  reason           failure reason, present only for the failed phases
  first_seen_at
  last_seen_at
  attempt_count
  stream_position
```

Keyed by `credential_id`. Built from the same credential event stream the
runtime projection already consumes, on the same checkpointed refresh
mechanism, so it needs no new transport, no application database table, and
no OpenBao read.

A row is created when the aggregate enters a non-terminal or failed phase and
deleted when it reaches `Active`, `Revoked`, or `Destroyed`. Deletion, not a
completed flag: the set must stay proportional to work in flight rather than
to every credential ever created. That property is the entire reason to have
this read model instead of scanning.

`first_seen_at` and `attempt_count`, rather than a bare age, are what make it
actionable. Age alone cannot separate a write still retrying inside a live
command handler from one abandoned by a crashed process, so an age-only
predicate races in-flight work.

There is no `rotation_failed` phase because there is no such state. A
RotationFailed event collapses the aggregate back to `Active`
(`state.rs:155-164`, which returns `active.clone()`), which is deliberate:
rotation failure is a recovery transition, not a dead end, unlike
`WriteFailed`. The cost is a blind spot. A rotation that failed and was never
retried is indistinguishable from a credential that never rotated, in the
aggregate and therefore in any state-derived row. Closing it means counting
RotationFailed events as they pass rather than reading them off state, which
is why `attempt_count` is specified as event-driven and is the one field this
read model cannot recompute from a snapshot alone.

#### Never shown to end users

This read model is operator-facing only. It has two display classes, and the
distinction matters because not every incomplete phase hides a credential.

```text
no publicly visible credential exists yet
  -> write_requested, write_failed
  -> nothing is rendered anywhere: no list row, no detail page, no count

a publicly visible credential already exists
  -> rotation_pending, destroy_requested, cleanup_failed
  -> the credential still renders under its own product status
     (`rotating` per ADR#0065 section 3, `cleanup_pending` per the product
     states in UI_ACCEPTANCE)
  -> but this read model's operational fields (attempt_count,
     stream_position, reason, first_seen_at) are never rendered
```

The reason for the first class is the one that settled the write ordering: an
intent is not a credential. `WriteRequested` records that the platform is
trying to create something, not that it exists. Rendering it would advertise a
credential whose secret may not be in OpenBao, which is exactly the failure
that writing the intent before the secret exists to prevent.

The product states in UI_ACCEPTANCE are not this read model and must not be
sourced from it. `saving` and `failed` describe a credential the user just
acted on, scoped to that request and that session. They are not a
platform-wide listing of everything stuck.

#### Surface

```text
GET /-/credentials/incomplete
  -> admin token, the same guard as /-/credentials/recovery/status
  -> metadata only: never secret material, never a raw credential value
  -> filterable by phase and by minimum age
```

```text
gateway.credential.incomplete.count        gauge, labeled by phase
gateway.credential.incomplete.oldest_age   gauge, seconds, labeled by phase
```

The count gauge labeled by phase is also the missing backlog signal named in
Remaining Work item 8: `phase=cleanup_failed` is the orphan cleanup backlog,
which today has no worker and no gauge.

#### Acceptance

- a credential that appends WriteRequested and never activates appears in the
  read model within one refresh interval;
- it disappears on activation, without a separate cleanup pass;
- a WriteFailed credential is listed with its reason and stays listed, since
  nothing can currently move it forward;
- no user-facing response contains a row, a count, or any field derived from
  this read model;
- the refresh reads only the credential event stream, with no OpenBao read,
  so unlike OpenBao-to-DB reconciliation this is not blocked on
  `SecretStoreList` and is buildable today.


## API Key Platform

Nothing is implemented. No bearer key, signed key, keyspace, or
`ApiPrincipal` code exists in the repository. `API_KEY.md` holds the full
design (data model, key format, create and verification flows, keyspaces,
identities, permissions, rate limits, rerolling, revocation) and is the
design of record; this section records only the build order and the
acceptance bar.

Signed mode is the strongly recommended default and the primary build
target per [ADR#0050](./docs/adr/0050-signed-first-caller-authentication.md);
bearer is the policy-bounded compatibility tier.

Every decision this section depends on is ratified:

```text
ADR#0046  project-anchored resource names; environment is an attribute
ADR#0048  one-time plaintext exposure; no escrow in any form
ADR#0050  signed-first posture; client-generated keys only; Ed25519/ES256
ADR#0051  fully bound single-use request tokens; NATS KV replay store
ADR#0062  key format, HMAC verifier construction, pepper custody
ADR#0063  keyspace model, keyspace policy, root key bootstrap
ADR#0064  permissions vocabulary, built-in roles, delegation, rate limits
ADR#0065  rotation grace, first-release audit set
```

### Commands to implement

The design behind each of these is in `API_KEY.md`; the ADR beside it is
where the reasoning lives. This list is the build surface, not a second
copy of the model.

Names are verb + noun per
[ADR#0014](./docs/adr/0014-command-and-query-naming.md), and they are the
generated proto message names under `proto/trogonai/platform/`, so the
build surface and the wire contract cannot drift into two vocabularies.

```text
CreateApiKeyspace               ADR#0063  policy boundary and its fields
UpdateApiKeyspacePolicy                   environment and tier are immutable
DeleteApiKeyspace                         refused while active keys remain

CreateApiKey                    ADR#0062  format and verifier digest
RerollApiKey                    ADR#0065  grace window; revoke has none
RevokeApiKey
ChangeApiKeyPolicy              ADR#0064  delegation ceiling checked here
ExpireApiKey                    ADR#0065  lazy, on first denied use

AddApiKeyPublicKey              ADR#0050  client-generated pairs only
RevokeApiKeyPublicKey           ADR#0065  swap bounded at 2 active
```

Verification is deliberately not on this list. It appends no event and
changes no state, so it is not a command: bearer verification is a
constant-time compare against the peppered digest
([ADR#0062](./docs/adr/0062-api-key-format-and-verifier-construction.md)),
and signed-request verification is a signature check plus an atomic
check-and-record against the replay store
([ADR#0051](./docs/adr/0051-fully-bound-request-signing.md) section 4).
Neither produces a domain event, because successful verification is a
metric rather than an audit fact
([ADR#0065](./docs/adr/0065-api-key-rotation-grace-and-audit-set.md)
section 6) and a denial is an audit fact rather than a state change.
`ExpireApiKey` is the single exception, and it is a command precisely
because it is the one thing a verification can durably record.

Two ordering constraints the build has to respect, both of which are
properties of this plan rather than of the design:

- the canonical target serialization in API_CONTRACTS lands before
  signed-request verification does, because the verifier and the SDKs have
  to agree on bytes before either can be tested against the other;
- tokens verify once at admission, and durable flows carry provenance
  onward per
  [ADR#0039](./docs/adr/0039-self-authenticating-event-provenance.md).
  Consumers never re-verify an expired caller token, so verification does
  not belong on any path downstream of admission.

Both modes return an `ApiPrincipal`, whose shape is in API_KEY.md as
amended by [ADR#0064](./docs/adr/0064-api-key-authorization-model-and-rate-limits.md):
one effective permission set, no `scopes`, resource scope as a
`ResourceScope` value object ANDed with permissions.

The seven required first-release audit facts are in
[ADR#0065](./docs/adr/0065-api-key-rotation-grace-and-audit-set.md)
section 5. Two of them, `api_key.denied` and
`api_key.signed_request_replayed`, are what the two unfirable alerts in
`devops/openbao/runbooks/alerts.md` are waiting on, so item 6i below closes
those alerts.

### Acceptance

- bearer verification is constant-time, and the checksum rejects malformed
  keys before any lookup;
- signed verification checks signature, expiry, skew, nonce, payload
  digest, replay record, and the canonical target;
- a target mismatch returns the verifier's computed canonical target so an
  SDK author can diff it;
- API keys never directly return raw provider credentials;
- the delegation ceiling is a checked invariant, not a documented rule:
  create and patch fail with `permission_denied` when the resulting key's
  effective permissions or resource scope exceed the acting principal's,
  and root keys are not exempt;
- rate limits attach to keys and projects in the first milestone;
  `identity` and `route` stay declared but unenforced, so adding them later
  is a policy row rather than a schema migration;
- rate-limit counters use NATS KV compare-and-set, fail open on ordinary
  keyspaces and closed on management keyspaces, and emit a distinct metric
  either way so a fail-open window is visible;
- the replay store, unlike the rate limiter, fails closed in every case.

## Testing Strategy

The implemented slice carries unit tests for value objects, state
transitions, and idempotency conflicts, plus integration tests for the
static, in-memory, and OpenBao adapters, projection refresh, and cache
invalidation. 909 tests run in `trogon-gateway`.

Covered:

- delivery policy validation (`runtime_delivery_policy.rs`,
  `injection_location.rs`, `runtime_service_id.rs`, plus resolver-level
  enforcement tests in `runtime_projection.rs`);
- allowed-host matching (`allowed_host.rs`, covering wildcard scope, case
  and trailing-dot normalization, port stripping, and the fail-closed
  absent-host case);
- no plaintext in logs, via a capturing tracing subscriber that runs a full
  resolve/rotate/revoke cycle with a canary plaintext and asserts the canary
  never reaches the layer, including when the error and the resolved
  material are logged with `?` and `%`;
- no plaintext in traces, by the same test: spans and events go through the
  same `tracing` layer, and the redaction lives in the `Debug` impls of
  `SecretString` and `SecretVerifier` rather than in a formatter, so it
  cannot be bypassed by a different exporter;
- no plaintext in metrics: no metric in the credential path takes a secret
  as a label or a value, and `SecretString` exposes no `Display`;
- a denied host cannot be bypassed;
- an unauthorized service identity cannot resolve credentials.

Not covered, each blocked on the feature it tests:

```text
verifier digest construction            (API key platform)
wire-format checksum rejection          (API key platform)
pepper rotation dual-version window     (API key platform)
canonical target vectors, both          (API key platform, and the one
  transports                             item ADR#0051 requires before
                                         signed verification ships)
signed-request verification             (API key platform)
signed request replay is rejected       (API key platform)
delegation ceiling is enforced          (API key platform)
rate limiter fails open on ordinary     (API key platform)
  keyspaces and closed on management
create saga with DB intent and outbox   (sagas)
cleanup worker                          (worker does not exist)
no plaintext in outbox                  (outbox does not exist)
```

The canonical target vectors are the one entry above that is not blocked on
the feature it tests. The serialization is fully specified in API_CONTRACTS
and the vectors are a fixture file, so they can be written and shared with
SDK authors before any verifier exists, which is the order ADR#0051 section
2 asks for.

Failure injection, none of it written:

```text
DB intent write fails
OpenBao write outcome unknown
DB activation succeeds but outbox publish fails
cleanup worker fails
provider revocation fails
client loses one-time API key response
```

Load tests, none of them written: gateway cache hit rate, cache miss
pressure on OpenBao, rotation invalidation latency, revocation latency,
idempotency ledger contention, OpenBao read/write throughput.

## Remaining Work

Ordered by dependency. Each item's blocker is stated because several look
like design work and are actually blocked on a consumer that does not
exist: the crate denies warnings and has no lib target, so a value object
or enum with no in-crate caller is a hard compile error, not dead weight.

### 1. Persistence and operations

Nothing below exists outside the event stream; the gateway has no database
dependency and no vault, operation, or audit-fact concept.

- persistence for vaults, credentials, versions, operations, idempotency
  records, and audit facts;
- pending-operation caps (the only cap today is the incidental
  one-pending-write-per-credential rule from the aggregate design);
- TTL handling for pending operations (today the only expiries are the
  blanket 24h idempotency bucket age and the recovery worker's stuck
  window);
- the full idempotency record shape above (the current KV record carries
  only fingerprint, status, and response);
- `CredentialState` and `OperationState` as guarded state machines, and the
  `OperationId` and `VaultId` value objects, all of which acquire their
  first caller here;
- audit facts as a distinct concept from domain events.

### 2. Public management API

Nothing public exists. The only surface today is the internal
admin-token-gated per-source API under `/-/credentials`.

- the vault-shaped public API specified under API_CONTRACTS;
- `IntegrationState` as a guarded machine on the integration aggregate.
  `RuntimeIntegrationStatus` in the runtime projection already carries six
  of its seven states, missing only `reconnect_required`, but it is not the
  same type: the projection's status is derived from credential state and
  overwritten wholesale on each refresh, so it has no transition to guard.
  Promoting it would put write-side rules on a read model.

### 3. Sagas, cleanup, and reconciliation

Per the section above: DB intent, outbox, resubmission flow, orphan
cleanup, tombstones, and async revoke. Needs a `SecretStoreList` operation
before OpenBao-to-DB reconciliation is possible at all.

The incomplete provision read model is the exception in this group: it reads
only the credential event stream, so it does not wait on `SecretStoreList` or
on the application database, and it is the prerequisite for knowing how large
the rest of this work actually is.

### 4. OpenBao production hardening

- bind service identities to the five policies. The policy files and their
  verification script exist; nothing binds an identity to them, so the
  scoping is a design rather than a control. This is the single largest
  operational gap.
- a declarative apply (Terraform or equivalent) instead of the manual loop
  in the policies README;
- provision an audit device, without which break-glass access cannot be
  audited and the ADR#0063 section 6 root-key recovery path cannot be used
  in production;
- run the restore drill specified in
  `devops/openbao/runbooks/backup-and-restore.md`, which needs a cluster.

### 5. Runtime projection

- populate the delivery policy from the management API and the credential
  event stream; the enforcement path exists and is exercised only by tests
  until something writes a policy;
- expose the cache TTL policy through configuration;
- invalidate on the outbox projection event once the outbox exists;
- decide fallback behavior for an OpenBao outage per credential kind.

### 6. API key platform

Per the section above. `ApiKeyState`, `SignedPublicKeyState`, `ApiKeyId`,
`ApiKeyspaceId`, `ApiKeyKind`, `ResourceScope`, and `IdentityId` all land
here; no key issuance surface exists today, so there is nothing for them to
transition or identify yet.

Every design question is now ratified (ADR#0046, ADR#0048, ADR#0050,
ADR#0051, ADR#0062 through ADR#0065), so what remains is build order rather
than open design:

```text
6a  keyspaces and policy
    ApiKeyspaceId, the policy value objects, project creation making
    `management` and `default`. Everything else needs a keyspace to live
    in, and the policy fields decide what the issuance paths may do.

6b  canonical target serialization and its shared test vectors
    Required by ADR#0051 section 2 to land before signed verification.
    Independent of 6a, so it can run in parallel.

6c  signed key issuance
    AddApiKeyPublicKey / RevokeApiKeyPublicKey, the fingerprint, the
    2-active-key rotation bound. No secret is minted, so this path needs
    neither the pepper nor the one-time display and is the shorter of the
    two tiers.

6d  signed verification
    The verifier against 6b, plus the NATS KV replay store with atomic
    conditional create and fail-closed behavior. Not a command: it appends
    no event, so it lands as a verification path rather than a decider.

6e  self-hosted bootstrap
    Register the root public key from deployment configuration,
    idempotent by fingerprint. Needs 6a and 6c; unblocks running a
    self-hosted deployment with no other auth path.

6f  bearer issuance and verification
    The wire format, the CRC32C checksum, the pepper resolved at boot
    through the secrets service, HMAC verification, one-time display, and
    reroll with its grace window. Deliberately after the signed tier:
    ADR#0050 makes bearer the compatibility tier, and building it second
    keeps the management API from having a bearer-authenticated era.

6g  authorization
    Built-in roles, direct_permissions, ResourceScope, and the delegation
    ceiling as a checked invariant. Needs a principal from 6d or 6f to
    check against.

6h  rate limits
    NATS KV compare-and-set counters, per key and per project, with the
    fail-open/fail-closed split and its metric.

6i  audit facts
    The seven required facts. Emitted as each producing path lands rather
    than as a trailing phase, since two of them are the signal for the
    alerts in item 8.
```

The one non-design blocker is item 1: audit facts are specified as distinct
from domain events, and nothing persists them yet.

Separately, `a2a-auth-callout`'s transitional HMAC-registry `ApiKey`
(`credentials/api_key.rs`, already marked deprecated, preferred last behind
OIDC and mTLS in `CredentialSource`) is the one place an API-key-shaped
concept exists in code today. It is not an ancestor of this platform and
shares nothing with it beyond the name; retiring it belongs to the A2A
stack's own migration, not to this build order. Worth stating so nobody
tries to grow it into the platform.

### 7. UI

Per UI_ACCEPTANCE. Depends on the public API existing first.

### 8. Alerting

Six of nine alerts have a signal to fire on in
`devops/openbao/runbooks/alerts.md`; none are deployed, and wiring them
into a monitoring backend is separate work. The most recent to gain one was
repeated OpenBao write failures, closed by
`gateway.credential.store.write.failures`.

Of the remaining three, the two API-key alerts, suspicious verification
failures and signed-request replay attempts, now have a specified signal
source: `api_key.denied` and `api_key.signed_request_replayed` in the
ADR#0065 audit set. They are no longer blocked on a design decision, only
on item 6i shipping, and item 6i names them as its reason for landing
incrementally rather than last.

One gap remains with no signal at all: orphan cleanup backlog, which has no
worker and no backlog gauge.

Also open: ratify the revocation latency target as an alert on
`gateway.credential.revocation.latency`, which is measured but not
alerted on.

## Open Decisions

Beyond the `DECISION NEEDED` blocks inline above and the four extraction
questions in
[ADR#0061](./docs/adr/0061-credential-platform-extraction-boundary.md):

- which OpenBao auth method each service should use;
- which credential kinds ship in the first UI;
- which providers get first-class validation on rotate;
- the default idempotency TTL;
- the default pending credential TTL;
- whether idempotency records stay in NATS KV or move to the control-plane
  database (the same question as ADR#0061's Q3).

Owner boundary, metadata backend, path convention, cache TTL, revocation
latency target, signed-key algorithm, the signed-first posture, request
signing binding, and the production seal are decided in ADR#0046 through
ADR#0052.

The API key platform has no architectural decisions left. Key format,
verifier construction, pepper custody, keyspaces and their policy fields,
root key bootstrap, the permissions vocabulary, built-in roles, delegation
enforcement, rate-limit subject kinds and counter store, rotation grace,
and the first-release audit set are decided in ADR#0062 through ADR#0065.
The three things deferred there (customer-authored roles, identity-level
and route-level rate limits, and a dedicated counter store) carry stated
trigger conditions, so they are deferrals rather than undecided design.

What remains open there is scoping, listed in API_KEY.md's own Open
Questions: which identity kinds ship first and whether identity is required
on a key, what replaces the internal `/-/credentials` admin API, concrete
first-milestone pending-operation caps, whether SDKs verify the wire-format
checksum before sending, and whether policy is patchable in the first
release or create-time only. None of them blocks starting item 6.

## Definition Of Done

The credential platform is done when:

- raw provider credentials are not in the application database;
- public API responses are metadata-only after one-time display;
- idempotent retries cannot create duplicate pending records;
- OpenBao paths are generated from validated domain values;
- every midway saga failure has a recovery path;
- gateway runtime resolution is authorized and cached;
- cleanup is idempotent and observable;
- API keys are split between verifier-only bearer keys and signed keys;
- signed keys do not require Trogonai to store caller private keys, on any
  path including bootstrap and break-glass recovery;
- no key can be issued with more authority than the principal that issued
  it, proven by test rather than asserted in documentation;
- a self-hosted deployment can reach its first authenticated request
  without a plaintext credential ever crossing the boundary;
- UI states hide distributed-system details from users;
- runbooks exist for stuck, leaked, orphaned, and missed-projection
  scenarios;
- tests prove redaction, authorization, retry, cleanup, rotation, and
  revocation behavior.

## Notable code findings not tied to a single section

These were confirmed while verifying the sections above and are recorded
here since they affect more than one section and are easy to lose track of:

```text
Datadog is a declared SourceKind with zero admin API routes.
CredentialStatus::Pending and ::Expired are declared but never produced.
RuntimeIntegrationStatus variants other than Active are declared but never
  produced outside test code.
write_failed is a terminal decider state with no recovery transition,
  unlike rotation_pending's RotationFailed-to-active recovery.
CredentialKind::parse() has no match arm for "webhook_token" even though
  CredentialKind::WebhookToken exists and has an as_str() of the same name.
The internal /-/credentials admin API has no per-owner authorization
  check; the shared admin token is the only authorization boundary, and
  owner_id is trusted as caller-supplied.
```
