# Connector Control API

`trogonai.connections.control.v1alpha1.ConnectionControlService` defines product
consent and disconnect workflows. It is a schema, not a deployed endpoint. The
console exposes its methods through generated unary ConnectRPC clients under
[ADR#0018](../adr/0018-connectrpc-gateway-for-browser-product-surfaces.md). The
annotated service binds to NATS micro under
[ADR#0016](../adr/0016-protobuf-rpc-over-nats-micro-binding.md); browser access
requires an explicit gateway method allowlist.

This surface does not expose aggregate `BeginAuthorization`, `ActivateConnection`,
claim, grant or credential commands. The [resource-parent contract](./connector-resource-parents.md)
continues to own typed create/list/get metadata envelopes. This service adds the
product workflow and filtered status projection around those resources.

## Authentication and request boundary

The gateway authenticates a human session and forwards verified operator-linked
authority through the existing bus authentication binding. Requests contain no
session principal, token, callback URI, browser binding, state nonce, signing key,
PKCE verifier, capsule, authorization code or client secret. Those values come
from trusted context or custody. Supplying an arbitrary parent is not authority.

The gateway and service reject a control request above 32 KiB before decoding;
status/list responses are bounded to 1 MiB. The authorization URL has a separate
16 KiB field bound. Unknown input fields, duplicate singular/oneof fields,
duplicate JSON keys and malformed wire data are rejected before admission,
rather than ignored as another version of an idempotent intent.

Every mutation requires strict `Origin` validation, the authenticated session and
the gateway's CSRF defenses. All methods authorize the exact project parent and
logical owner from live service facts before reading or writing. An execution
permit does not grant connection administration or permission to inspect its
onboarding status. Unknown and unauthorized resource identities return the same
external failure shape. Project collection permission failures can use a fixed
permission-denied result without probing a child id.

Creation uses `StartConnectRequest.parent`. Existing resources use complete typed
resource names. The dispatcher compares any HTTP route identity with the typed
payload and checks the authoritative immutable parent. A Connection never moves
because its logical owner changes, and a request cannot change that owner.
Project-owned requests require the owner project to equal custody. Workspace and
person owners are rejected with `OWNER_KIND_NOT_ENABLED` until their rollout
phase is explicitly enabled.

## Methods and domain mapping

ConnectRPC uses the generated method path
`/trogonai.connections.control.v1alpha1.ConnectionControlService/{Method}`. The
NATS micro endpoint uses the configured service group plus that exact method
name, following [ADR#0016](../adr/0016-protobuf-rpc-over-nats-micro-binding.md). No parallel REST or hand-written JSON client is defined.

| Method | Caller input | Service behavior |
| --- | --- | --- |
| `StartConnect` | Custody parent, immutable release pin, owner, OAuth or MCP target, optional scopes and mutation id | Allocate stable ids, reserve Pending Connection, open custody capsule and commit `BeginAuthorization`; return consent status and the first transient redirect |
| `Reconnect` | Connection name, observed revision, expected authorization generation and additional scopes | Keep Connection id, owner, app, release and account identity; create a fresh attempt pinned to the current configuration and generation |
| `Disconnect` | Connection name, observed revision and mutation id | Commit `RevokeConnection(DISCONNECT)` for a live Connection or abort a Pending one, then let lifecycle workers reconcile cleanup |
| `CancelConnect` | Attempt name, observed attempt revision and mutation id | Commit `CancelAuthorization` only before exchange; abort an unactivated fresh target and reconcile its capsule and staged material |
| `GetAuthorizationStatus` | Attempt name | Return sanitized phase, Connection name, expiry and source revision, without a redirect or OAuth material |
| `GetConnectionStatus` | Connection name | Return folded lifecycle, generation when established, health reasons and open cleanup obligations |
| `ListConnectionStatuses` | Project parent, typed filter and page | Return only authorized rows in that parent, in opaque ConnectionId byte order |
| `GetControlRequest` | Parent, action and original request id | Recover only the authenticated caller's durable mutation receipt after a timeout or lost reply |

A release fixes its account subject kind through the reviewed identity probe.
`StartConnect` accepts only methods the release declares. Required scopes come
from the release; requested optional scopes must be an allowed subset. The
service normalizes scopes once and rejects unrecognized input. A request cannot
select a different token endpoint, issuer, app secret or credential destination.

ProviderApp selection is custody project first, then platform, with ambiguity
refused. A project app must share custody. For MCP, the supplied canonical HTTPS
resource is input to the existing untrusted-discovery sandbox and egress checks,
not permission to fetch a private address. The service selects the issuer from
validated pinned metadata and fails if no trusted account identity strategy is
supported. Caller-supplied issuer and discovery bytes are not accepted here.

## Consent and callback handling

The service commits a durable mutation receipt before making external calls and
uses stable Connection/Attempt ids and domain command ids during recovery. It
returns a successful consent response only after the Pending reservation and
`BeginAuthorization` are durably established. A disconnect or standing change
between preparation and delivery prevents delivery of a usable redirect.

Custody generates the verifier and returns only its S256 challenge. The service
constructs the authorization URL from pinned endpoints and server-derived OAuth
parameters. The environment registers one exact HTTPS callback per platform app;
local development uses a separate app. There is no `return_to`, callback, issuer
override or post-login destination input. After the callback the gateway computes
the console destination from the verified attempt and outcome. It never follows
a destination supplied by the browser or provider.

The initiating principal and browser-binding cookie must match at callback,
using the opaque keyed bindings of
[ADR#0094](../adr/0094-oauth-authorization-protocol.md). The gateway installs a
Secure, HttpOnly, `SameSite=Lax`, `__Host-` cookie for the short-lived browser
binding and rejects a callback without the authenticated session. It preserves
an existing binding while that browser has open attempts, rather than rotating
it on every tab and invalidating another consent. The original attempt expiry
still applies independently.

The callback remains a plain top-level HTTPS `GET` route outside ConnectRPC. It
is the documented Origin-check exception, protected by single-use signed state,
PKCE, exact issuer checking and the principal/browser bindings. No new callback
method accepts codes in this API.

`BrowserAuthorizationRedirect.authorization_url` is a transient sensitive reply.
The service may deliver it only to the original authenticated principal and
browser binding while the attempt is AwaitingConsent and unexpired. It never
appears in a durable receipt, event, snapshot, status, page token, error, audit
fact or application cache. Mesh transit carrying it requires the receiver-sealed
request/reply protection of [ADR#0082](../adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md)/0083. Supporting that protection in the
gateway bridge is a release gate; ordinary unsealed or persisted micro traffic
is insufficient.

All control responses use `Cache-Control: no-store` and
`Referrer-Policy: no-referrer`. Gateway and service request/response body logging,
query logging and automatic tracing of redirect values are disabled. The
browser navigates immediately and keeps the action in memory only, never in
local storage, session storage, IndexedDB, telemetry or service-worker caches.
The provider authorize page and callback set their own browser history; the
callback's server-computed `303` removes the callback query from the current
address. This does not claim that a server can erase an earlier history entry.

## Idempotency and lost replies

The client generates a fresh cryptographically random, lowercase UUIDv4 per intended action
and preserves `ControlMutation` and all intent fields on retry. It never
refreshes `issued_at`, expected revision or requested scopes under the same id.
The server's idempotency key binds authenticated tenant and principal, exact
parent, method and request id; no principal value comes from protobuf input.

On an unseen key, `issued_at` must be no more than ten minutes old or thirty
seconds in the future. This bound prevents a retired receipt from turning an
old request into a fresh action. The server atomically claims the key, records
a digest of the validated intent and stable target ids, and fences a single
workflow owner before side effects. Equal retries return that record; different
intent under the key returns `IDEMPOTENCY_CONFLICT`. Authorized duplicates check
the receipt before running concurrency checks against a newer revision.

The intent digest is SHA-256 over the UTF-8
[RFC 8785 canonical JSON](https://www.rfc-editor.org/rfc/rfc8785.html) projection
below. This is a defined bookkeeping projection, not a claim that arbitrary
protobuf encodings are canonical. Every listed member is present; an
inapplicable value is JSON `null`, and an inapplicable scope list is `[]`.

| Member | Canonical value |
| --- | --- |
| `profile` | Literal `connector-control-intent-v1` |
| `action` | Exact `ControlAction` enum name |
| `project` | Exact custody ProjectId string |
| `requestId` | Lowercase UUIDv4 string |
| `issuedAt` | Object with `seconds` and `nanos`, each a decimal string without redundant zeroes |
| `expectedRevision` | Positive decimal string, or `null` for start |
| `resource` | Canonical Connection/Attempt public name for existing targets, otherwise `null` |
| `expectedAuthorizationGeneration` | Positive decimal string for reconnect, otherwise `null` |
| `owner` | Start-only object with `kind` (`project`, `workspace` or `person`) and exact `id` string |
| `target` | Start-only object with `release`, `digestAlgorithm`, `digestValue`, `method`, `resourceUri` and `optionalScopes` |
| `additionalScopes` | Normalized, unique reconnect scopes sorted by UTF-8 byte order |

The target's `release` is its canonical public name; `digestValue` is unpadded
base64url of the pinned raw digest. `method` is `oauth` or `mcp`, `resourceUri` is
the validated MCP URI or `null`, and `optionalScopes` is the normalized unique
scope list in UTF-8 byte order. `digestAlgorithm` is the exact validated
algorithm string. Do not Unicode-normalize opaque ids or add service-generated
ids, callback data, authorization URL, browser cookie, authorization headers or
ambient timestamps to this projection. The authenticated caller is bound by the
idempotency-key scope separately. Reject invalid normalization and duplicate
normalized scopes before hashing. The digest stays in service bookkeeping and
is not a public response or permission receipt.

An ACCEPTED receipt means the request was durably registered and recovery is
responsible for it. COMMITTED means the intended domain transition committed.
FAILED records a sanitized terminal failure. OUTCOME_UNKNOWN means the service
must reconcile before deciding whether the transition committed; it is never
permission to allocate another attempt, exchange another code or call the
provider again. Recovery examines the durable domain command receipt, not an
in-memory success flag. Receipt targets may be allocated before their streams
exist, so callers poll `GetControlRequest` until COMMITTED before reading them.

Terminal receipts remain available for at least seven days; unresolved ACCEPTED
and OUTCOME_UNKNOWN receipts are retained until settled. After expiry an old
request is rejected as `REQUEST_EXPIRED`, not executed again. A request id is
never intentionally reused. Audit retains the ordinary immutable intent facts,
not the transient authorization URL or browser cookie.

A retry of `StartConnect` or `Reconnect` returns the existing receipt and current
sanitized attempt status, with no redirect. The existing custody capsule has no
challenge-readback operation, and the control service does not persist the
authorization URL or verifier merely to replay a response. If the initial
redirect was lost, the client explicitly cancels that attempt and starts a new
intent with a new id after cancellation succeeds. Once exchange began, it waits
for reconciliation instead of cancelling or replacing the attempt blindly.

## Concurrency and lifecycle

`StartConnect` has no expected revision. Reconnect and disconnect use the observed
Connection stream revision; cancellation uses the observed Attempt revision.
Reconnect also requires the observed positive authorization generation. A stale
request fails with `REVISION_CONFLICT`; refreshing a revision represents new
intent and requires a new request id.

Only one unfinished authorization attempt may be admitted per Connection.
Service-owned coordination uses a durable compare-and-set/fenced reservation
around the authoritative target, including pre-genesis preparation. Another
attempt receives `ATTEMPT_IN_PROGRESS`; timeout does not steal the reservation.
Recovery releases it only after a durable terminal attempt or a proved failed
preparation with its capsule cleanup scheduled. Generation checks remain the
final protection against activation races with reconnect or disconnect.

Cancellation is accepted only in AwaitingConsent and CallbackClaimed. It is
refused in Exchanging and OutcomeUnknown, including a race where exchange
committed first. Repeating the same accepted cancellation returns its receipt.
An already-Cancelled attempt can satisfy a new authorized cancellation after its
current revision is checked; other terminal phases are not converted to
Cancelled. A cancelled reconnect leaves the prior Connection authorization
unchanged.

Disconnect is a terminal local intent. It does not claim that the provider
revoked tokens or all material is already destroyed. A Pending target becomes
Aborted; any exchange already underway must fail activation and leave its cleanup
obligations rather than resurrect it. A live target becomes Revoked and live
admission/grant checks refuse further use under the bounded-live revocation
contract. Already-Revoked or Aborted targets can satisfy a new authorized
disconnect as a no-op after the current revision check. Provider cleanup and
profile/material erasure remain observable obligations in `ConnectionStatus`.

## Status and filtering

Authorization status is available to the initiating principal or an authorized
owner administrator. Request receipts additionally require the original
principal. Each read reauthorizes live parent/owner access. No read returns the
session initiator binding, raw account identity, provider error text, capsule,
staged bundle handle or provider response.

`ConnectionStatus.lifecycle` is the source fold's lifecycle at `stream_revision`,
not a guarantee of current usability. `observed_at` identifies the projection
observation; service reads reject a projection older than their configured bound,
which cannot exceed the existing 330-second authorization freshness ceiling.
Mutations and conflict resolution read authoritative revisions, not this cached
status. Release, app, project standing and access still participate in live use
admission.

The list filter supports an OR of lifecycle values and an optional exact
ConnectorId; those dimensions are ANDed. Omission means all authorized values
within the same parent. Arbitrary expressions, raw provider ids, names, free-text
account searches and cross-project queries are unsupported. The response echoes
the applied filter and validates every returned row. It uses the existing page
size default of 50 and maximum of 200. An integrity-protected cursor binds caller,
tenant, parent, filter, bytewise ConnectionId ordering and read snapshot. Each
page reauthorizes; a changed context rejects the cursor.

## Failure mapping

Service faults use the canonical `google.rpc.Status` error channel of [ADR#0016](../adr/0016-protobuf-rpc-over-nats-micro-binding.md),
translated mechanically to Connect's canonical code. `ControlFailure` is a typed
detail. Its reason contains no provider input, and messages are fixed templates.
Sanitized terminal workflow failures also appear in status/receipt records.

| Control reason | Canonical code |
| --- | --- |
| `INVALID_ARGUMENT` | `INVALID_ARGUMENT` |
| `RESOURCE_NOT_AVAILABLE` | `NOT_FOUND` |
| `REVISION_CONFLICT`, `IDEMPOTENCY_CONFLICT` | `ABORTED` |
| `ATTEMPT_IN_PROGRESS`, `NOT_CANCELLABLE`, `TERMINAL_CONNECTION`, `UNSUPPORTED_METHOD`, `ACCOUNT_ALREADY_CONNECTED`, `ACCOUNT_MISMATCH`, `AUTHORIZATION_EXPIRED`, `OWNER_KIND_NOT_ENABLED`, `SCOPE_NOT_ALLOWED`, `REQUEST_EXPIRED` | `FAILED_PRECONDITION` |
| `PROVIDER_UNAVAILABLE`, `UNAVAILABLE` | `UNAVAILABLE` |
| `PROVIDER_REJECTED`, `AUTHORIZATION_FAILED` | `FAILED_PRECONDITION` |
| `OUTCOME_UNKNOWN` | `UNKNOWN` |
| `RATE_LIMITED` | `RESOURCE_EXHAUSTED` |
| `PERMISSION_DENIED` | `PERMISSION_DENIED` |

Missing/invalid gateway authentication maps to `UNAUTHENTICATED` before domain
dispatch. It is not an observable attempt failure. Rate-limit and unavailable
responses may include bounded `retry_after`; this advises when to recover the
same request, not permission to repeat a provider effect. No raw upstream status,
body or exception text is inserted into error details.

A terminal FAILED receipt remains failed on retry; trying the action again is
new explicit intent with a new id. If admission failed before a receipt existed
and the original issuance window has expired, recover against the authoritative
receipt store before creating new intent. Never refresh the old request's time
or interpret backoff advice as an exemption from its issuance window.

## Standards and verification boundary

The exact callback and open-redirect defenses follow
[OAuth Security BCP](https://www.rfc-editor.org/rfc/rfc9700.html#section-2.1).
The S256 verifier/challenge boundary follows
[PKCE](https://www.rfc-editor.org/rfc/rfc7636.html#section-4.2). Those standards do
not prescribe this product's receipt retention or concurrency protocol; those
are conservative platform choices that require implementation tests. HTTP method
names alone do not make these RPCs idempotent; the service-owned receipt and
domain concurrency checks provide the intended repeated-request behavior, in
the sense of [HTTP semantics](https://www.rfc-editor.org/rfc/rfc9110.html#section-9.2.2).

Schema validation proves shape and local equality only. Before exposing this
service, prove live authorization, session/browser binding, receiver-sealed
transport, gateway logging redaction, idempotency races and crash recovery,
generation conflicts, cancellation/exchange races, filtered cursor isolation and
cleanup behavior in service and browser integration tests. Generated descriptors
and the service declaration do not establish that those paths are running.
