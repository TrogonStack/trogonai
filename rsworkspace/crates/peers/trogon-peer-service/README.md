# Peer application orchestration

`PeerService` applies generated enrollment, heartbeat and detach requests.
Enrollment resumes the same live connector without replacing its attachment;
a competing live connector is refused. Expired or detached attachments advance
the generation through the core `Attach` decision. Different native identities
produce different peers. A response read that observes a competing replacement
fails instead of returning that connector's assignment.

`PeerApplication` applies generated send, status, preparation, application
acknowledgment and roster requests. Its explicit `reconcile` method processes
only named message IDs from one sender, with a validated work limit of at most
64. Its returned count means attempted obligations, including deferred intake;
it is not a delivery or settlement receipt. Sending records the sender's durable
obligation and returns without reconciliation, a peer scan or a native turn.

## Dependency and authority contract

The caller supplies a tenant-scoped store, policy and clock. Application methods
also require injected content, prepared-presentation, identity-generation and
catalog ports. There are no production defaults. `ScopedPeerStore::tenant`
describes the adapter's immutable scope; every stream operation must enforce
that scope. The service refuses a mismatched authenticated owner before I/O.

The admitted subject boundary supplies `TrustedPeerOwner`. Request headers,
model inputs, state documents and received receipts cannot establish authority.
The selected connector profile maps that owner to an `Agent` command principal
whose ID is the trusted caller ID.

Implement `CommandAuthorizer<PeerAccess>` for operation and peer-read grants.
An application checks this policy before accessing each protected scope.
Implement the corresponding core command authorizers as well: `Enroll`, `Attach`,
`Heartbeat`, `Detach`, `SendMessage`, `IntakeMessage`, `DisposeMessage`,
`PrepareBatch`, `AcknowledgeBatch` and `SettleMessage`. The core exposes read-only
command accessors for inspecting real authorization inputs. No permissive
command authorizer is installed by this crate. `CommandExecution` authorizes
again before its own history reads and guarded append.

All writes use core decisions and the existing runtime's storage and CAS
orchestration. Replay has a 4096-event ceiling. A command may re-read and
re-decide once after a backend-classified CAS conflict; infrastructure failures
are not automatically retried. Read-side folds use runtime bounded-read
primitives and core evolution. The physical I/O bound requires a store adapter
that implements genuinely bounded reads.

## Content and presentation ports

`ContentSeal` must bind the authenticated tenant, authorized opaque reference,
exact UTF-8 byte size and stable semantic fingerprint to the actual text. The
fingerprint must survive location and nonce changes, and must use a reviewed
keyed construction rather than expose an unkeyed content hash. `ContentOpen`
must authenticate that same binding before returning text. Confirmed absence or
erasure is distinct from an infrastructure or key-service failure.

New sends must fit a single escaped context before content sealing. Admission
uses the generated message ID and authorized provenance, reserving the maximum
valid batch-ID width and nine fractional timestamp digits. This is a byte bound;
the reservation timestamp is never persisted. Actual acceptance time is sampled
after sealing and attachment checks, so a heartbeat during sealing does not make
the send command use an earlier time. Existing accepted keys still reach the
content port and core semantic conflict checks before returning the original
receipt.

ADR 0083 sealing and key-service infrastructure are not implemented here.
There is no plaintext fallback. `DisposableContentFixture` exists only in tests;
its in-memory text and allocated semantic tokens do not implement sealing or
production cryptography.

Preparation chooses a fitting oldest eligible prefix using `trogon-peer-context`
and its escaping and byte budgets. It persists the exact payload and rendered
frame under the proposed stable batch ID before executing the guarded decision.
Cache insertion must be durable, immutable and reject a different existing
record. Inserted candidates must remain tracked. A known loser is marked for
orphan collection; an uncertain append retains its candidate because it might
already be the winner. Retention and authorized privacy erasure belong to the
injected presentation implementation.

The wire response contains `PreparedMessage` values, not rendered context.
The cache therefore retains those exact authorized payloads alongside their
rendered frame. Replay verifies the winning batch, payload metadata and shared
renderer output against that immutable record before returning the cached
payload. An incompatible frame, missing record or erased presentation returns
the existing batch with `RETRY_UNAVAILABLE` and no messages. It never rewrites a
prepared disposition or creates an acknowledgment. A prepared winner survives
later message expiry and source-content removal.

When preparation encounters unavailable source content before any fitting
assignment exists, it returns `RETRY_UNAVAILABLE` without a batch or a write.
Reconciliation is responsible for recording an authorized terminal disposition.

## Reconciliation and receipt meanings

Reconciliation loads accepted sender history from the scoped store and derives
`AcceptedMessage` from that history. It applies recipient intake or authorized
disposition through the core. Overflow defers intake and leaves the sender
obligation unresolved. Offline connector state does not delete durable inboxes.
Infrastructure failures preserve retryability.

Settlement uses `AuthoritativeReceipt` derived from the recipient's authentic
winning history. Wire receipts are not an input. Preparation receipts permit
sender settlement while the recipient still retains its pending capacity.

Application acknowledgment requires the recipient owner and current live
attachment. It stops replay and releases recipient capacity once. It is separate
from sender settlement, broker acknowledgment, host-context insertion,
understanding and task completion.

## Roster and integration

`PeerCatalog` is a trusted projection port for authorized collaboration membership.
A page is bounded, sorted and strictly after its peer-ID cursor. The application
authorizes each candidate before its history read. Page limits and continuation
apply to catalog candidates, so a restricted page can be empty and still have a
continuation. A full last page can be followed by an empty terminal page.

Catalog membership, lease liveness and historical activity remain separate.
The live roster excludes expired and detached attachments. Rows use service
observation time and the actual lease expiry. Catalog membership, durable inboxes
and historical activity survive expiry and detach. There are no live all-peer
watchers or additional workflow owners.

Broker ingress, the executable adapter, reconciliation and heartbeat scheduling,
native detach hooks, content and presentation infrastructure, retention workers,
and real host acceptance must be composed by the runtime owner. These tests
prove application behavior over the existing `InMemoryStore` through typed
adapters. They do not establish a broker or native-host exchange.

## Verification

Run the focused service, core and shared-renderer tests from the Rust workspace:

```sh
MISE_EXEC_AUTO_INSTALL=false mise exec -- cargo test -p trogon-peer-service -p trogon-peer -p trogon-peer-context
```
