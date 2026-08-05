# Credential Platform Spec (Draft)

## Status

This file is a draft of the six Phase 0 deliverables from PLAN.md
(CREDENTIAL_LIFECYCLE, API_CONTRACTS, AUTHORIZATION_MATRIX,
OPENBAO_OPERATIONS, RUNTIME_PROJECTION, UI_ACCEPTANCE). It is a draft for
ratification, not a decision record. Nothing here should be read as final
until the DECISION NEEDED blocks below are resolved by the people who own
this project.

Every claim in this file about what "exists today" was checked directly
against the trogon-gateway source in this repository (grep and full file
reads of the relevant modules), not copied from SECRET_STORE.md or
API_KEY.md, since PLAN.md's own status section notes that the first
implementation slice already shipped and that those two design documents
predate it. Anywhere a section depends on one of PLAN.md's Required
Decisions or Decisions Still Needed, that question is called out inline as:

```text
DECISION NEEDED: <question>
recommendation: <recommendation, grounded in PLAN.md>
```

No such question has been silently answered by writing this file; each one
still needs sign-off. PLAN.md itself says these six deliverables "can begin
as sections in one file, but they should become separate files before
implementation grows," so this single file is the intended starting point,
not a final structure.

## Credential Lifecycle (CREDENTIAL_LIFECYCLE)

There are three separate lifecycle layers in play: the event-sourced decider
snapshot (implemented), the per-version CredentialStatus used by the
SecretStore adapters (implemented, and distinct from the decider), and the
broader CredentialState/IntegrationState/OperationState machines from
PLAN.md Phase 1 (not built at all). They are easy to conflate because they
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
This gap is exactly what PLAN.md's Phase 1 `resubmission_required` state is
meant to close, and that state does not exist in code today.

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
CredentialStatus values, or should Phase 1 wire them up (for example,
Expired for a TTL-driven per-version expiry) or remove them so the enum
only lists what the store layer actually produces?
recommendation: keep them reserved through Phase 1. PLAN.md's Phase 1
CredentialVersionState additions (resubmission_required,
revocation_requested, destroy_requested, destroyed, cleanup_failed) already
plan a richer version-state set that will likely absorb or replace this
enum; removing the placeholders now and reintroducing similar ones during
Phase 1 is churn with no benefit in between.

### Layer 3: Phase 1 planned states (not built)

None of the following exist as Rust types or proto messages anywhere in the
repository today; they are PLAN.md Phase 1's plan, quoted here so this file
is a complete lifecycle picture rather than only the implemented part:

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
and CredentialStatus; they are not replacements for either. PLAN.md's
Product Model implies CredentialState is closer to what users actually see
(draft, saving, ready, failed, ...; see UI_ACCEPTANCE), while the decider
snapshot and CredentialStatus stay internal machinery.

DECISION NEEDED: does the decider's write_failed map onto CredentialState
failed as a recoverable state, or does Phase 1 need the
resubmission_required transition added to evolve() first?
recommendation: add the resubmission_required transition to the decider
before wiring CredentialState failed to any retry UI. Today, offering a
"retry" action against a write_failed credential would offer an action the
decider has no path to accept.

DECISION NEEDED (Required Decision): owner boundary, one of workspace,
organization, project, tenant, or user.
recommendation: treat owner_id as the top-level tenant/account boundary,
distinct from workspace. PLAN.md's own Phase 1 value-object list already
separates `OwnerId` from `WorkspaceId` as two different planned types (and
notes OwnerId is "pending the Phase 0 decision" while "today only a
credential-scoped CredentialOwnerId exists"). Grepping the codebase confirms
`WorkspaceId` does not exist anywhere yet, only `CredentialOwnerId`
(a validated, non-empty, length-bounded string). Keeping owner_id as the
coarser tenant boundary and introducing workspace_id as a nested concept
under it matches what PLAN.md has already implied without contradicting it.

DECISION NEEDED (Required Decision): first credential metadata backend,
one of Postgres, NATS KV, or the existing control plane store.
recommendation: application database (Postgres), per PLAN.md's own
Implementation Defaults ("credential backend -> application database for
metadata, OpenBao for raw provider credential material"). This is already
the documented default; nothing in the current code contradicts it since
the gateway has no database dependency yet at all (Phase 3: "the gateway
has no database dependency and no vault, operation, or audit-fact concept
at all").

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
matching PLAN.md's Phase 3 note about "the blanket 24h idempotency bucket
age."

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

These are this API's own ad hoc strings, not PLAN.md's public error-code
vocabulary (validation_failed, permission_denied, idempotency_conflict,
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

DECISION NEEDED: should the internal /-/credentials admin API adopt
PLAN.md's public error-code vocabulary now, or keep its own strings until
Phase 4 replaces it?
recommendation: keep the internal API's own codes. PLAN.md is explicit that
this is "the internal admin-token-gated per-source API under
/-/credentials," a bridge, not the public surface; adopting the public
vocabulary now would imply guarantees (vault_id, operation_id) that this
API does not have.

### Public API, PLAN.md Phase 4 (nothing built)

Every endpoint below is unbuilt. There is no vault, operation, or
resubmit-secret concept behind any of them today.

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

Idempotency rule per write command, following PLAN.md's Phase 3 contract
(`unique(owner_id, command_namespace, idempotency_key)`, or
`unique(owner_id, command_namespace, target_resource_id, idempotency_key)`
for targeted operations):

```text
POST /v1/credential-vaults
  namespace credential_vault.create, scoped by owner_id + key (no target
  yet since the vault_id does not exist before this call)

PATCH .../{vault_id}, POST .../archive, POST .../restore
  namespace credential_vault.update / .archive / .restore, targeted by
  vault_id

POST /v1/credentials
  namespace credential.create, targeted by credential_id if the client can
  pre-assign one, otherwise scoped by owner_id + key alone

PATCH /v1/credentials/{credential_id}
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

GET endpoints
  read-only, no idempotency key required
```

Plaintext rule per PLAN.md's Response Rules ("forbidden: raw secret") and
Definition of Done ("public API responses are metadata-only after one-time
display"): every endpoint above is metadata-only except the two that
legitimately hand back new secret material, and even those only display it
once:

```text
POST /v1/credentials          -> returns the new secret value once, on the
                                  first non-replay execution only
POST .../rotate                -> returns the new secret value once, same
                                  one-time rule
POST .../resubmit-secret        -> receives plaintext in the request; the
                                  response should stay metadata-only (there
                                  is nothing new to display back)
every other endpoint            -> never plaintext
```

Error codes, taken verbatim from PLAN.md's Phase 4 list, mapped per
endpoint by what can plausibly fail there:

```text
POST /v1/credential-vaults            validation_failed, permission_denied,
                                       idempotency_conflict
GET .../credential-vaults[/{id}]      validation_failed, permission_denied
PATCH .../{vault_id}                  validation_failed, permission_denied,
                                       idempotency_conflict
POST .../archive, .../restore          validation_failed, permission_denied,
                                       idempotency_conflict,
                                       pending_operation_exists

POST /v1/credentials                  validation_failed, permission_denied,
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
GET /v1/operations/{operation_id}     validation_failed, permission_denied
```

Two error codes in PLAN.md's list, `host_not_allowed` and
`runtime_service_not_allowed`, describe runtime resolution failures rather
than management-API failures; they belong to the delivery-policy checks
described in RUNTIME_PROJECTION (allowed_hosts, allowed_runtime_services),
not to any of the endpoints above.

DECISION NEEDED (Decisions Still Needed): must a replay of the same
idempotency key re-serve the one-time plaintext from create/rotate, or does
replay only ever return metadata once the value has been shown once?
recommendation: replay must never re-serve plaintext, even for the exact
same idempotency key and fingerprint. Store only a metadata-only response
snapshot in the idempotency record, per PLAN.md's own Phase 3 Acceptance
Criteria ("Response snapshots never contain raw secrets or one-time key
material"); treat the one-time display as a side channel that fires exactly
once, on the original non-replay execution, never on
IdempotencyDecision::Replay.

DECISION NEEDED (Decisions Still Needed): is one-time display escrow
allowed at all?
recommendation: no escrow. A lost one-time response requires
resubmit-secret (credentials) or reroll (API keys), matching PLAN.md's
Failure UX section verbatim ("lost one-time API key response -> offer
reroll") and preserving the Definition of Done guarantee that public API
responses are metadata-only after one-time display.

DECISION NEEDED (Decisions Still Needed): what default idempotency TTL
should apply on the public API?
recommendation: keep the 24 hour figure already implemented in the internal
API's KV ledger (`CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE = Duration::
from_secs(24 * 60 * 60)`) as the ratified default, rather than choosing a
new number for the public API alone; the two surfaces will likely share a
ledger once Phase 3's persistence work lands.

DECISION NEEDED (Decisions Still Needed): which providers get first-class
validation on rotate (PLAN.md Phase 6 Rotation Saga step "Validate if
provider supports validation")?
recommendation: none today, and that should be stated plainly rather than
implied. `SecretVerifier` exists as a value-object wrapper
(`secret_store/secret_verifier.rs`) but it only wraps a string; there is no
per-provider validation call anywhere in the eleven wired sources today.
Prioritize adding first-class validation for the providers that already
have a working write/rotate/revoke path through the internal admin API
before adding it for anything not yet wired.

## Authorization Matrix (AUTHORIZATION_MATRIX)

PLAN.md Phase 5 names five policies: control_plane_write, gateway_read,
lifecycle_worker_cleanup, audit_read, break_glass_admin. Today there is
exactly one OpenBao identity in code: a single static bearer token
(`OPENBAO_TOKEN`), sent unconditionally as `X-Vault-Token` by
`OpenBaoSecretStore::authorize()` for every operation, regardless of which
Rust component is calling (the admin API's command handler, the recovery
worker, the runtime projection resolver). There is no AppRole, no
Kubernetes auth, no per-service policy anywhere in the code; "no policy
files exist anywhere in the repo yet (no HCL, no Terraform)" from PLAN.md
Phase 5 is accurate as read.

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

DECISION NEEDED (Decisions Still Needed): what can support/audit roles see
during incidents?
recommendation: audit_read should see credential metadata (status,
fingerprint, last-rotated timestamp, owner) and operation/audit-fact
history, never plaintext and never raw OpenBao paths. Break-glass access
should be its own short-lived elevated grant, not a standing widened
audit_read policy.

## OpenBao Operations (OPENBAO_OPERATIONS)

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
Planned fields that do not exist yet, per PLAN.md Phase 5:

```text
workspace_id           (needs the Phase 0 owner boundary decision)
integration_id          (needs integration records)
operation_id            (needs Phase 3 operation records)
credential_version_id   (needs its own convention, since KV v2 metadata has
                        no native per-version attribution)
```

### Auth method (implemented today)

A single static bearer token, sent as `X-Vault-Token` unconditionally for
every operation (`authorize()` in `openbao_secret_store.rs`), loaded from
the `OPENBAO_TOKEN` environment variable. No AppRole, Kubernetes auth, or
certificate auth is configured anywhere in the code. This matches PLAN.md
Phase 0's own framing: "the prototype uses a static dev token; that is not
a production answer."

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
           version passed in; nothing in the runtime calls destroy() yet
           (PLAN.md Phase 2: "no runtime path invokes the store-level
           destroy yet"). Logs the reason via tracing::info! only; there is
           no audit-fact record
```

This lines up with PLAN.md's own list of when OpenBao should be read
(create/rotate, runtime cache miss, startup/reconnect, explicit refresh,
cleanup/reconciliation): the implemented resolver reads on cache miss, and
the recovery worker reads metadata() to recover stuck activations. The
"during cleanup" pattern has no code yet since cleanup itself is unbuilt
(see RUNTIME_PROJECTION and PLAN.md Phase 6).

DECISION NEEDED (Required Decision / Decisions Still Needed): ratify
`trogonai/{owner_id}/credentials/{credential_id}` as the published OpenBao
path convention and mount name, or replace either before more sources are
wired.
recommendation: ratify the path convention as-is; it is deterministic and
reconstructible purely from validated domain values, and PLAN.md's own
Definition of Done already requires exactly this ("OpenBao paths are
generated from validated domain values"). For the mount, move off the
generic default `"secret"` to a dedicated mount name (for example
`"trogonai"`), since a shared, out-of-the-box `"secret"` mount gives
OpenBao policies no boundary to scope against; a dedicated mount is the
natural coarse-grained policy boundary the five named policies in
AUTHORIZATION_MATRIX need.

DECISION NEEDED: should revoke() keep soft-deleting every version 1 through
current, or move to targeting only the version(s) a caller intended to
revoke?
recommendation: change revoke() to accept an explicit version (or version
range) once Phase 1's revocation_requested state exists, instead of
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
(`from_credential_state()` at line 216 and `active_runtime_projection()` at
line 278, both above the `#[cfg(test)]` boundary at line 1288/1409) shows
only `Active` is ever produced in production code.
`from_credential_state()` maps the decider's `Active` and `RotationPending`
cases to `RuntimeIntegrationStatus::Active`, and returns `None` (no
projection at all) for `Missing`, `PendingWrite`, `WriteFailed`, and
`Revoked`. `Disabled`, `Archived`, `Deleted`, `Pending`, and `Failed` are
declared but never constructed outside test helper code; they are
placeholders for the Phase 1 CredentialState/IntegrationState machinery,
not wired to anything today.

This is worth flagging against PLAN.md's own Phase 7 framing, which says
"fail-closed behavior on revoked or disabled credentials all exist." The
revoked half is real: a Revoked event removes the credential from its
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

### Planned fields (PLAN.md Phase 7, none of these exist today)

```text
RuntimeCredentialProjection (missing fields)
  workspace_id
  allowed_hosts
  allowed_runtime_services
  injection_locations
  cache_policy
```

`RuntimeIntegrationProjection` only carries key, owner_id, status, version,
and the per-kind CredentialRef map today; none of the five planned policy
fields exist on it.

DECISION NEEDED (Required Decision / Decisions Still Needed): first default
cache TTL and revocation latency target.
recommendation: ratify the existing 300s ttl / 30s jitter as the default
rather than choosing a new number, since it is already implemented and
exercised by tests. Set the revocation latency target at ttl + jitter worst
case (330s) until Phase 6's outbox-driven invalidation exists; once outbox
invalidation lands, tighten the target, since revocation will no longer
need to wait out the cache TTL.

DECISION NEEDED: should cache invalidation stay whole-cache-clear on every
projection merge, or move to per-CredentialRef invalidation now, ahead of
Phase 6's outbox?
recommendation: move to per-CredentialRef invalidation now. `apply_state`
already knows the specific CredentialRef inside the Active/RotationPending
state it just merged, so invalidating just that entry (the way revoke
already does) is a small change, and it avoids flushing unrelated
integrations' cached material on every unrelated rotation.

## UI Acceptance (UI_ACCEPTANCE)

Nothing described in this section exists in the UI or public API today.
PLAN.md Phase 9 states plainly that "the console app is a single-route
scaffold with no data layer, and the credentials proto packages define no
RPC services a browser client could call." Everything below is acceptance
criteria for work that has not started, written now so Phase 4/9 build
against agreed behavior.

### Product states (PLAN.md Product Model)

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

### Vault list columns (PLAN.md Credential Vault List)

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

Given any credential state
When the user inspects it through the UI
Then the user never sees OpenBao paths, mounts, policies, or saga/outbox
  terminology, only the product states listed above.

These map directly onto PLAN.md's own Phase 9 Acceptance Criteria: the user
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
