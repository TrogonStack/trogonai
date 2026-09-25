---
number: "0068"
slug: provisioning-sagas-and-owned-resource-cleanup
status: draft
date: 2026-09-24
---

# ADR#0068: Provisioning Starts in the Owner Stream and Ends with Accounted Effects

## Context

A credential, connection or webhook can span an event stream, OpenBao and a
provider API. Those systems cannot participate in one atomic transaction.
Creating the remote object first risks an untracked secret or registration;
marking the local object active first risks exposing incomplete configuration.
A failed request does not prove that the provider failed to create anything.

This record completes the event contracts from
[ADR#0066](./0066-secret-service-and-connection-boundaries.md) and
[ADR#0067](./0067-webhook-ingress-and-delivery.md). Its deliverable is durable
protobuf events and their supporting value types. Command, query, transport and
material exchange schemas, generated bindings and Rust implementation are
outside this change. Design choices are settled here; the `draft` status
records the separate maintainer signoff process in
[ADR#0000](./0000-adr-process.md), not unresolved architecture alternatives.

## Decision

### Reserve locally before touching an external system

Each resource has a small authoritative aggregate stream. Its reservation
records project, identity, immutable non-secret definition and provisioning
reference. A newly reserved resource is unusable. `ProvisioningStarted` then fixes the
operation purpose, deadline, ownership and dependency graph in that same
stream. Both facts must be committed before any external side effect. A crash
between reservation and start leaves a local reservation that can be expired
without remote cleanup. No resource ID is reused after abandonment.

The reusable `trogonai.provisioning.v1alpha1.ProvisioningEvent` is embedded in
credential, connection and webhook event envelopes. It shares the owning
stream's event ID, timestamp and revision. It is not a second coordinator
stream with a competing commit decision. Expected-revision appends serialize
activation, cancellation, attempts and cleanup. Operation caches and background
job indexes are rebuildable projections; their absence cannot erase an obligation.

An immutable plan records each effect before it can occur. An effect has an
exact resource identity, pinned adapter, operation, stable random idempotency
key and one ownership class: newly owned, already owned or borrowed. Typed
receipts point to immutable domain facts with the exact version or external
object identity. A protected locator is stored in custody and represented by a
reference. Tokens, credentials, provider response bodies and secret-derived
hashes cannot be recovery identifiers in the stream.

### Reconcile ambiguity rather than inventing success or failure

`EffectAttemptStarted` is committed before dispatch. It records a monotonically
increasing fence and bounded worker lease. Results from stale fences cannot
change authoritative state. The remote service may still finish an old request,
so its potential effect remains in the plan regardless of lease expiry.
`EffectObserved` distinguishes applied, absent, unknown and a verified borrowed
resource. Transport timeouts are unknown.

Automated creation requires an adapter contract that can recover the exact
owned object after a lost response. The supported mechanisms are provider
idempotency with a known retention period, a deterministic object identity, or
an atomic conditional version write. Read-only admission is available for
preexisting resources. Similar labels, material hashes and account membership
are insufficient ownership evidence. A provider that cannot identify and settle
its possible creations is restricted to bringing an existing credential or
registration; the platform does not promise automatic creation for it.

Idempotency keys are stable across attempts and bound to caller, project and
non-secret intent. A new key is not an automatic escape from an ambiguous
operation. After a provider's deduplication window expires, creation stops until
reconciliation or compensation settles the previous intent. OAuth exchanges,
refresh-token rotation and dynamic credential issuance use this rule too. When
a provider cannot recover a rotated token, record reconnect-required and retain
the cleanup obligation for its grant. Never regenerate another account's access
or claim that losing the response undid the refresh.

Before final absence is accepted, `EffectQuiescenceConfirmed` accounts for all
dispatched attempts through its fence. Evidence must establish a remote fence,
a documented adapter settlement bound including eventual visibility, no dispatched
effect, or closure of an OpenBao conditional write. Cancellation, a local timeout,
lease expiry and a single lookup returning 404 do not establish quiescence.
Creation cannot restart after quiescence closes the operation for compensation.
Removal cannot be compensated back to an active resource: partial deletion is
irreversible, so its remaining cleanup obligations continue until confirmed.

### Keep OpenBao attribution atomic with material

The secrets service writes an internal KV v2 data envelope containing the
operation identity and material in the same compare-and-set write. Operation
identity contains no secret and is copied into the observed domain receipt.
KV v2 `custom_metadata` is path-scoped; it is not a per-version transaction
record and cannot prove which operation wrote a particular version.

Before initial creation, reserve the backend path with a non-secret marker and
CAS zero, then write material conditionally against that marker's version.
Marker creation and cleanup are planned effects too. Rotation reserves the
current backend version and serializes candidate writes for the credential.
A successful candidate write advances the backend version, closing duplicate
writes against its old CAS value. If cancellation wins before a candidate
write, a non-secret fence write consumes the reserved version before cleanup.
The service quiesces attempts to create the initial marker as well, using a
bounded per-operation backend credential and settlement evidence. It cannot
purge metadata while an old CAS-zero request might still create the path.

Backend retention must preserve the active version, permitted overlap and all
unresolved attribution. Before each write, the service verifies that KV v2
version limits and automatic deletion cannot prune any such version. It raises
service-owned retention limits safely or refuses the write; repeated failed
rotations cannot age the active credential out of backend history.

Destroy candidate versions irreversibly before purging exclusively owned path
metadata. During a failed rotation, preserve the previous active version and
the path metadata it needs. Purge the whole path only for an abandoned initial
credential or final removal after every use, version and pending writer has
been accounted for. KV v2 soft deletion is reversible and is not cleanup
completion. A compact event tombstone and scoped deduplication identity remain
so delayed messages cannot recreate the resource.

### Activate once, with explicit ownership transfer

`ProvisioningPrepared` records receipts for all required effects and verification.
It does not authorize runtime use. Domain activation rechecks current project,
parent, binding, credential and workload policy, then commits its exact receipts
under the owner's expected revision. That domain fact is the linearization
point. `ProvisioningCommitted` acknowledges it; if that acknowledgement is lost,
recovery reads the domain fact and cannot instead compensate a committed object.

A connection or webhook can provision a child credential. Parent activation
accepts verified prepared owned children; borrowed dependencies must already be
active. Runtime use stays closed until child activation is acknowledged or an
authoritative custody check observes adoption. The child knows its
parent operation before creating material. The parent's activation adopts exact
`ResourceReceiptRef` values, making the ownership decision atomically within
the parent stream. A subsequent child ownership-transfer fact is an acknowledgement,
not a cross-stream transaction. A child sweeper reads the authoritative parent
decision before deleting prepared material. If that stream is unavailable,
cleanup waits. It never infers abandonment from a stale projection or expired
parent lease. Parent compensation permanently prevents later adoption.

Borrowed credentials and registrations remain owned by their original resource.
A binding grants use, not deletion rights. Shared user credentials are borrowed;
per-subscription signing secrets and automatically created registrations are
owned. Removing a connection does not silently destroy a credential merely
because a role refers to it. Replacing bindings or configuration requires an
explicit ownership decision for the old and new resources.

### Cleanup obligations survive failure

Cancellation, deadline expiry or failed verification commits
`ProvisioningCompensationStarted`. This permanently closes activation. The plan
itself determines every possible cleanup obligation, even if a worker dies
before emitting individual `CleanupRequired` facts. Unknown effects are looked
up and settled. Newly owned resources are removed; borrowed resources and the
previous active version of a failed rotation are retained.

Cleanup is dependency ordered. Delete a remote registration or revoke a provider
grant before destroying the credential required to perform that cleanup. Each
remnant has its own stable cleanup ID, allowing version destruction, provider
revocation and owned metadata purge to be confirmed separately. A compensation
attempt records that ID as well as its step and fence. Cleanup authority is a
narrow platform obligation tied to the original ownership receipt; revoking
ordinary runtime use must not authorize continued business calls or erase the
ability to remove an owned object.

`CleanupConfirmed` requires exact ownership, quiescence and authoritative
absence evidence. `CleanupDeferred` retains a typed reason and retry time when
the provider, required authority or parent decision is unavailable. Exponential
backoff with jitter and operator escalation control load; retry exhaustion is
not permission to discard the debt. There is no terminal "failed but forgotten"
state. `ProvisioningCompensated` is legal only after every possible effect has
been settled and every derived obligation confirmed.

Deletion of an active resource first disables new use locally, then runs a new
removal operation with its adopted ownership receipts. Successful rotation also
creates a durable retirement obligation for superseded material, delayed only
by a recorded overlap deadline. The activation fact contains enough information
to rebuild that obligation if scheduling crashes. Removal may complete with a
tombstone only after all owned external resources and child operations settle.

"No trash" therefore means no untracked effect and no falsely completed cleanup.
An unavailable provider can leave an explicitly pending external object until
recovery is possible. The design cannot guarantee immediate physical deletion
while that provider is unreachable. Journals, ownership evidence and tombstones
are intentional retained data, not abandoned operational material.

### Background work is a projection of durable facts

Workers discover outstanding operations from stream events, then reconcile
against authoritative revisions. Periodic scanning repairs missed notifications
and stale job indexes. Pending operations, unresolved effects, deferred cleanup,
expiring leases and expired rotation overlaps remain discoverable after restart.
Stream retention and snapshot compaction must retain the complete unresolved
plan and ownership evidence. Compaction cannot remove active cleanup debt.

Source event publication is an outbox obligation derived from the committed
fact. Delivery and material writes are not retried by replaying historical events
as fresh commands. Duplicate notifications resume the same effect identity.
Artifact-first webhook ingress also needs ownership: extracted secrets and
sanitized payload artifacts belong to an admission reservation until the durable
receipt adopts them; a sweeper removes abandoned artifacts only after ruling out
an accepted receipt and late writers. Delivery retention expires payload bytes
without silently deleting the delivery outcome or its audit evidence.

## Failure-window contract

| Interruption | Durable authority | Recovery outcome |
| --- | --- | --- |
| Reservation before plan | Owner reservation | Abandon local reservation; no external work was authorized |
| Plan before dispatch | Started plan, no attempt | Resume or confirm no effect was attempted |
| Dispatch before provider response | Attempt and ownership identity | Reconcile exact object; unknown is not absence |
| Provider success before receipt | Immutable plan and provider attribution | Reconstruct receipt without another creation |
| Prepared child before parent activation | Child preparation and parent stream | Adopt once or compensate only after parent abort |
| Parent activation before child acknowledgement | Parent adoption fact | Acknowledge transfer; never sweep adopted material |
| Cancellation while old worker runs | Compensation decision and attempt fences | Settle late effects before confirming cleanup |
| Cleanup success before its event | Stable cleanup ID and original ownership | Recheck absence and append the same cleanup result |
| Rotation activation before retirement job | Activation and previous-version metadata | Rebuild retirement obligation; no orphaned old version |
| Provider outage during cleanup | Deferred obligation | Retry and escalate without claiming completion |

## Consequences

- Domains share effect accounting without sharing their credentials, lifecycle
  decisions or aggregate transaction boundaries.
- The event stream can reconstruct unfinished work; an operational database or
  queue is not a second write model.
- Provider capabilities constrain automatic provisioning. Unsupported recovery
  semantics are an admission failure, not an optimistic cleanup promise.
- Event schemas express the required evidence but do not implement workers,
  authorization, CAS, provider fencing or operational retention. Those require
  later runtime and fault-injection evidence in both deployment models.

## References

- [OpenBao KV v2 API](https://openbao.org/api-docs/secret/kv/kv-v2/): conditional writes, version destruction and metadata deletion.
- [AWS Builders' Library: Making retries safe with idempotent APIs](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/): request identity and late-arriving calls.
- [Azure Architecture Center: Compensating Transaction](https://learn.microsoft.com/en-us/azure/architecture/patterns/compensating-transaction): resumable, application-specific compensation.
- [Event contract reference](../reference/secret-and-connection-contracts.md)
