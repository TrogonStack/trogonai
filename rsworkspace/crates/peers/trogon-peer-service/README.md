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
orchestration. Replay has a 4096-event ceiling. A command without a caller-supplied
revision may re-read and re-decide once after a backend-classified CAS conflict;
infrastructure failures are not automatically retried. Read-side folds use runtime bounded-read
primitives and core evolution. The physical I/O bound requires a store adapter
that implements genuinely bounded reads.

### Enrollment and resume

When create-only enrollment conflicts with an existing stream, resume authorizes
`Attach` before reading that stream. It validates the history, rejecting more
than 4096 events and any unknown event type, then pins the attachment append to
the validated stream position. A concurrent authority event rejects the append
instead of allowing a replacement across unvalidated history. The command replay
also retains the 4096-event limit. Reusing a live attachment rechecks history
before returning it.

Stores receive bounded read requests, but their implementation may materialize a
larger response before the service rejects it. The replay ceiling therefore does
not guarantee a transport I/O bound. A longer valid history cannot resume through
this boundary, and an event this version cannot interpret cannot be assumed
irrelevant to ownership or attachment authority. Longer-lived histories need an
authorized checkpoint or read-model contract; widening the bound or silently
skipping unknown events does not provide that contract.

## Sealed-content collection

`ContentSeal` implementations must durably track every sealed candidate for
retention-aware collection, including candidates created for retries and sends
that do not return an acceptance receipt. Collection belongs to the content
backend and may reclaim a candidate only when authoritative message state
confirms that it is unreferenced. It must preserve referenced candidates and
bindings whose acceptance remains uncertain; a failed send or append error alone
does not establish absence of a reference.

This is a backend contract; this crate implements no content collector or
collection schedule. Authorized privacy erasure remains a separate backend
responsibility.

## Content and presentation ports

`ContentSeal` must bind the authenticated tenant, authorized `ClaimCheck`,
exact opened UTF-8 byte size and stable semantic fingerprint to the actual text.
The `ClaimCheck` digest verifies referenced stored bytes, including the sealed
envelope. The purpose-specific semantic fingerprint verifies opened-content
semantics independently of those stored bytes. The
fingerprint must survive location and nonce changes, and must use a reviewed
keyed construction rather than expose an unkeyed content hash. `ContentOpen`
must authenticate that same binding, verify stored-byte integrity, and verify
the opened byte size and semantic fingerprint before returning text. Neither
digest substitutes for the other or grants authority. Confirmed absence or
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

`trogon-peer-nats` supplies the authenticated RPC adapter and typed client,
including a concrete composition of these application surfaces and scoped
JetStream storage. The executable adapter, reconciliation and heartbeat
scheduling, native detach hooks, content and presentation infrastructure,
retention workers and real host acceptance remain runtime responsibilities.
This crate's tests prove application behavior over the existing `InMemoryStore`.
Its disposable fixtures are exposed through `test-support` for broker integration
tests; they do not supply production content or policy defaults.

## Verification

Run the focused service, core and shared-renderer tests from the Rust workspace:

```sh
mise exec -- cargo test -p trogon-peer-service -p trogon-peer -p trogon-peer-context --all-features
```
