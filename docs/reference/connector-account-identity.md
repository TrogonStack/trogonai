# Connector Account Identity

This reference defines the custody boundary for a trusted identity probe and the
lookup-key transition that preserves account claims. The messages are in
`secrets/exchange/identity.proto`. They specify service obligations; they do not
establish a running identity registry or key service.

## Authorized probe flow

An authenticated connection attempt pins its custody project, owner, issuer,
ProviderApp, ConnectorRelease and resource. Its staged bundle may be resolved
only by the registered trusted identity-probe worker. The worker executes the
release's fixed HTTP GET or GraphQL identity query and reads only its declared
fields. It accepts JSON strings for identity components, rejects missing required
fields, arrays, objects, nulls, numeric coercion and duplicate JSON member names,
and discards all undeclared response fields. Provider text cannot select a key,
owner, app, destination or normalization rule.

The worker submits `FingerprintAccountIdentityRequest` with the exact
`AttemptRef`, accepted probe audit correlation and authenticated-xkey-sealed
`AccountIdentityProbe`. The decoded probe repeats the attempt/stage binding,
definition digest, subject kind, correlation and observation time. Custody
authenticates the sender against the registered probe-worker class and its xkey,
verifies the inner and outer bindings, checks the immutable attempt and stage,
and reads its own live authorization state. The attempt must be open and
unexpired, and the observation must be after that stage was written, no later
than now, and at most 60 seconds old. Unknown fields and malformed sealed inputs
are rejected before identity lookup. Request and decoded input are bounded to
16 KiB; the decoded representation must also satisfy all field bounds.

An agent, supervisor, browser or arbitrary MCP server cannot call this operation.
The registered worker remains trusted code: authenticated sealing establishes
which worker sent the input, not that a hostile provider's response is true.
Reviewed probe origin, field selection, issuer and app binding provide that
trust boundary. No token, authorization code or raw identity appears in this RPC's
unsealed envelope, an event, public resource response, audit record or log.

## Exact identity framing

Each provider identity component is a nonempty, control-free UTF-8 string of at
most 512 characters and 2,048 bytes. It is preserved byte for byte: no trimming,
case folding, Unicode normalization, numeric conversion or inference from an
email, display name or token text. The reviewed probe must yield `account_id`.
Optional components are present exactly when the probe declares and returns
them. Missing declared identity fields fail; absence is never replaced with an
empty string. Display name and granted scope are not identity components.

Let `F(s)` be `u32be(length(UTF8(s))) || UTF8(s)`. Let `O(s)` be byte `0x00`
when absent and `0x01 || F(s)` when present. Integers are unsigned big-endian;
owner, app-scope and subject-kind tags are single bytes. The HMAC input is the
following concatenation, in this exact order:

```text
F("trogon/account-identity/v1")
u32be(lookup_generation)
F(verified_tenant_account_public_key)
F(custody_project_id)
u8(owner_kind) || F(owner_id)
F(connector_id)
F(exact_issuer)
u8(app_scope) || O(app_project_id) || F(provider_app_id)
O(canonical_resource)
u8(provider_subject_kind)
F(account_id)
O(subject_id)
O(organization_id)
O(workspace_id)
O(installation_id)
```

Owner tags are project `1`, workspace `2`, person `3`, with the exact identifier
from the corresponding owner arm. App scope is platform `1` or project `2`;
`app_project_id` is absent only for platform scope. `canonical_resource` is the
exact pinned resource identifier when present, including the protected MCP
resource. It is not the token endpoint or the provider response's issuer guess.
Subject-kind tags retain the `ProviderSubjectKind` enum numbers. All boundary
values come from authenticated service state, not extra probe response fields.
Release id, configuration digest, display label, token version and scopes are
excluded so a compatible release or credential rotation does not rename an
account. A changed identity boundary requires a new connection.

Custody computes the full, untruncated HMAC-SHA-256 with an independently random
256-bit key for each tenant/custody-project/generation. It does not reuse an
audit, token, state-signing, sealing, webhook or tenant content key. Callers
cannot select the key or generation. HMAC is the standard construction;
the framing and identity registry below are this project's protocol choices.
[RFC 2104](https://www.rfc-editor.org/rfc/rfc2104.html) defines HMAC, and
[RFC 4231](https://www.rfc-editor.org/rfc/rfc4231.html) provides HMAC-SHA-256
conformance vectors.

## Canonical claim and lookup aliases

The first allocation fixes a canonical `ProviderAccountFingerprint` and claim
id. The id is `g` followed by the fingerprint generation as unsigned decimal
without leading zeroes, `_`, then unpadded base64url of its 32-byte value. The
canonical generation and value never change while the claim is Held or Bound.
Adoption of an existing claim retains its recorded id and fingerprint; a
bootstrap verifies that binding from authoritative history rather than
renaming it to this new-allocation format.
A lookup alias is a private `(generation, HMAC value)` mapping to that same
canonical fingerprint and id. A newer lookup key therefore does not rename a
claim or create a false reconnect mismatch. Comparing two newly computed MACs
from different generations directly is invalid.

Custody owns one serialized identity registry per verified tenant and custody
project. Its authoritative state consists of the fenced key-ring epoch,
canonical records, generation aliases and outstanding attempt/stage receipts.
Alias material is pseudonymous metadata, not bearer authority. The connections
service remains the sole writer of the claim's Held/Bound/Released stream.

The registry must serialize an allocation and every alias update as one durable
operation. A supported event-sourced implementation uses one parent-scoped
stream, `StreamUnchanged` compare-and-append, and one event containing the
canonical decision, complete alias batch and receipt. The serving state must be
current through the committed revision; an eventually consistent projection
cannot authorize a new allocation. Separate KV `put`/CAS operations on individual
alias keys do not provide this guarantee. A different storage engine must provide
the same serializable transaction and fencing semantics before admission is
enabled. Bootstrap must reconcile the registry with authoritative claim and
outstanding-receipt history before it enters Stable; an empty newly created
index is not evidence that no claims exist.

Within the operation, custody computes aliases for every accepted lookup
generation and checks all of them. If any resolve to one canonical claim, it
uses that claim and installs any missing aliases atomically. If they resolve to
different claims, it returns `INTEGRITY_FAILURE` and quarantines the parent; it
never merges active claims. If none resolve, it allocates under the current
write generation and atomically installs the complete alias set. The epoch and
accepted-generation set are checked in the same operation. A concurrent writer
loses the expected-revision check and recomputes from current state; it cannot
reuse a stale empty lookup.

`AccountIdentityReceipt` is returned only after that operation commits. Its
canonical account, claim id, epoch, revision and attempt/stage are saved in
custody's registry. A retry with the same attempt/stage and canonical identity
returns the same decision while valid; a different identity under that binding
is an integrity failure. The receipt expires no later than the attempt.
Uncaptured receipts from an old epoch cannot authorize capture or hold.

Dispatch reads the authoritative receipt by exact attempt, stage version and
audit correlation before `CaptureGrantedAuthority` and `HoldProviderAccount`,
compares every account and parent field, and verifies that the epoch is still
admitted. A receipt from another attempt is rejected even for the same account.
Reconnect compares the resolved canonical
fingerprint with the connection's existing canonical fingerprint. A caller's
copy of a receipt is not proof. The ordinary claim `NoStream`/expected-revision
rules still choose the winner when attempts target one account; registry
allocation itself does not hold, bind, release or steal that claim.

The parent fence must be checked at the actual claim append boundary, under a
shared append permit serialized with fence acquisition. Checking before
enqueueing a command is insufficient: an old queued command must not append
after the fence's drain barrier. All connections-service writers participate,
including retries and recovery workers. If a backend cannot atomically exclude
those appends while acquiring the barrier, identity admission stays disabled.
This is an adapter obligation; a pure claim fold does not read another stream.

## Lookup-key rotation

Key rotation and alias cutover use the following parent-scoped states. No new
rotation begins while an earlier transition is unsettled. Stable operation uses
one lookup generation; a transition accepts at most the old and next generation.
Generations and epochs are positive, monotonic and never reused or wrapped.

| State | Required behavior |
| --- | --- |
| Stable | One write/read generation; ordinary atomic lookup and allocation |
| Preparing | Fence new allocations, capture/hold admission and receipt issuance; drain already admitted registry/claim operations and record their barriers |
| Backfilling | Install an independently generated next key and increment the epoch; keep allocation fenced while reindexing every Held/Bound canonical claim and outstanding valid receipt |
| Ready | Prove new-generation alias coverage against an authoritative claim/receipt inventory at the fence; conflicting or missing coverage refuses progress |
| Cutover | Atomically set the next generation as the sole lookup/write generation, then reopen admission at the new epoch |
| Retiring | Invalidate old-epoch receipts, confirm all old workers are fenced and recovery/backup policy cannot restore their authority, then destroy the old computation key |
| Quarantined | No identity allocation, capture or reconnect until an integrity/key-loss recovery establishes complete coverage or terminally releases affected claims |

Use a 90-day lookup-key cryptoperiod by default and begin the transition seven
days before expiry. A deployment may choose a shorter documented cryptoperiod.
Incomplete coverage does not silently extend a key past its configured limit:
identity admission remains fenced and operators are paged. These durations are
local operational defaults, not durations prescribed by NIST.

`ReindexAccountIdentityRequest` is limited to custody's authenticated migration
worker under the exclusive fence. It uses the canonical claim and an
authenticated read of the connection's existing erasable profile and immutable
identity boundary. The sealed `AccountIdentityReindexInput` binds the exact
connection/configuration, claim, canonical fingerprint, profile revision, epoch
and audit correlation. Repeating a caller-provided revision is not proof of a
profile read. Before adding a new alias, custody checks the reconstructed input
against an already admitted lookup alias and the authoritative canonical claim.
Reindexing never creates a new claim or changes a holder.

No new raw-identity journal is introduced for rotation. Existing profile data
may be used only while available and authorized. If a necessary tuple component
is missing, erased or was never retained, coverage remains incomplete. A fresh
trusted probe under a valid attempt may establish the alias; otherwise the
connection must reach its ordinary terminal state and release the claim before
that uncovered entry leaves the coverage set. The system never substitutes an
email, drops a tuple component, guesses identity or releases a live claim to
finish a rotation. Active non-person subjects whose required subject id is not
retained have this same fresh-probe requirement.

While allocation is fenced, an authenticated probe may backfill only an already
recorded canonical claim bound to that attempt's existing target or saved
attempt/stage receipt. It must verify an admitted old-generation alias and
cannot allocate a new canonical claim, change the tuple or authorize capture or
hold before cutover. A fresh consent round targeting that same existing
connection may perform this restricted probe. A generic new-account attempt
continues to receive REGISTRY_FENCED. During compromise quarantine, an old MAC
does not qualify as the required evidence.

Old canonical fingerprints remain valid opaque identities after their HMAC key
is destroyed, because current aliases resolve them without recomputing the old
MAC. Old aliases must not be reused to admit fresh identities after cutover.
On a suspected key compromise, stop use of that key immediately, fence identity
admission and quarantine the affected parent. Re-establish coverage from
authenticated original profiles or fresh trusted probes with independent key
material; a compromised MAC alone is not evidence. If the required evidence is
lost, ordinary terminal cleanup and a new consent are required.

Protect fingerprint keys inside custody's key-management boundary, with access
audit, explicit activation/deactivation/destruction state and inventory of
backups. Routine wrapping-key rotation does not change HMAC identity. This
lifecycle follows the key-management principles in
[NIST SP 800-57 Part 1 Rev. 5](https://csrc.nist.gov/pubs/sp/800/57/pt1/r5/final);
the alias and fence algorithm is a project-specific design. Revision 6 was
still an initial public draft when this contract was researched.

## Recovery, retention and erasure

| Failure point | Required recovery |
| --- | --- |
| Before registry commit | Retry at the current epoch; no identity receipt exists |
| After commit, reply lost | Read the bound attempt/stage receipt; do not allocate a second canonical claim |
| Between registry receipt and claim hold | Keep the canonical aliases and receipt in the migration coverage set; normal attempt cleanup reconciles the claim |
| During backfill | Resume from durable epoch/revision; a count without verified claim coverage is insufficient |
| After cutover, before key destruction | Serve only the new epoch; old workers stay fenced |
| Lost/rolled-back registry or uncertain restore | Quarantine and reconcile against authoritative claim history before allocation; never treat an empty index as an empty account set |

Purge the raw provider profile and its revisions through the existing erasure
workflow. Do not extend profile retention merely to make key rotation easier.
Aliases for Held/Bound claims must continue preventing duplicate admission even
when raw profile data was erased. Once the holding connection is terminal, the
claim is Released, no unexpired receipt or admitted operation can reference it,
and a fence verifies those facts, its lookup aliases may be purged according to
the tenant's pseudonymous-metadata retention policy. Purge must cover index
history, snapshots and recoverable copies before claiming physical deletion.
Pseudonymous canonical claim history follows its existing event retention
policy. A later connection may allocate a new canonical generation after the
old Released claim's aliases are purged; it cannot collide with an active claim.

Tenant deletion purges the registry and profiles and destroys all lookup keys
after the existing revoke/cleanup ordering. A restored old key or snapshot never
reactivates an old epoch. Restore requires an external monotonic recovery fence
that outlives the restored data; if that fence is unavailable or stale, identity
admission stays disabled.

## Generic MCP and typed failures

MCP authorization discovers authorization servers and binds access tokens to a
protected resource. It does not supply this platform's reviewed provider-account
identity contract. Consequently, a generic authenticated MCP connection must
have a release-pinned, reviewed HTTP identity probe yielding the required stable
tuple. The implementation must not infer an account from a bearer token, a tool
description, a resource URL, an unverified JWT payload or arbitrary tool output.
Without the probe, return `PROBE_UNSUPPORTED` before claim hold and activation.
Anonymous connections that require no provider account use their existing
anonymous authorization path and do not fabricate a fingerprint. This identity
requirement is a platform inference from the transport-level scope of the
[pinned MCP authorization specification, 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization).
The [MCP authorization profile](./connector-mcp-authorization.md) records that
explicit authorization pin separately from transport and session versions.

`AccountIdentityFailure` preserves the shared custody category and adds a closed
reason for trusted callers. Unknown and unauthorized attempts both use DENIED.
Unsupported or malformed probes are non-retryable denied operations;
unavailability and a registry fence use UNAVAILABLE with bounded backoff;
throttling uses THROTTLED; disabled/compromised key use is KEY_DENIED; alias
conflicts, mismatched bindings and inconsistent history are INTEGRITY_FAILURE.
No failure returns a provider identifier, response body, key locator or raw
input. Public callers receive the product facade's mapped error, not these
trusted custody details.

## Conformance obligations

Implementations must exercise fixed HMAC/framing vectors, component-boundary
collisions, absent versus empty input, Unicode byte preservation, every tenant,
owner/app/resource boundary and wrong-stage/wrong-caller rejection. Rotation
tests must cover racing allocations, stale epochs, conflicting aliases, missing
profiles, Held/Bound and outstanding-receipt coverage, crash at every state,
restore rollback, key compromise and old-key retirement. Two-generation lookup
must resolve a previously Bound claim to the same canonical fingerprint and
claim id; no test may equate independent MAC generations directly.

These requirements need runtime and multi-replica proof. Schema-instance checks
validate message shape and local parent/category constraints only.

[Synthetic identity vectors](/connector-fixtures/connector-account-identity-v1.json)
provide fixed framing bytes, full HMAC values, canonical claim ids, an RFC 4231
library control and required rotation scenarios. The fixture key and identifiers
are public test inputs, never production material.
