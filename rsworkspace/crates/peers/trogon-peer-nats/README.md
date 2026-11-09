# Peer NATS routing and storage

This crate provides the peer RPC binding, typed client, connector permission
profile and tenant-scoped event-store adapter. The runtime supplies an
authenticated TLS connection, credential admission and application dependencies.
Collaboration and command authorization remain injected application policies.

The request grammar is:

```text
shared-account fixture: {tenant}.peer.v1.{caller}.PeerService.{Method}
tenant account:         peer.v1.{caller}.PeerService.{Method}
```

`PeerIngress` is configured with an explicit account mode and tenant. It rejects
other tenants, malformed principals, unknown methods, and subjects outside this
grammar. Call it on the received NATS subject before decoding the request body.
In a tenant account, the configured tenant is the account's identity. In a shared
fixture, the broker's publish grant must fix both tenant and caller positions.
Parsing a string supplied by an untrusted client does not authenticate it.

`PeerPermissions::for_connection` produces exact method publish grants for the
configured caller and a subscription grant for:

```text
_INBOX.{tenant}.{caller}.{connection}.>
```

The connection token is a transport identity, separate from durable peer
enrollment. A trusted credential issuer chooses or verifies it. The profile
grants no JetStream, KV, consumer, raw data stream, or arbitrary publication
access. The broker must actually apply these grants; producing the profile does
not prove enforcement.

Before publishing a response, the service uses `PeerReplySubject::parse_for` to
check an exact reply subject under the authenticated tenant and caller:

```text
_INBOX.{tenant}.{caller}.{connection}.{nonce}
```

This confines delivery to that caller's namespace. It does not establish that
the connection token belongs to the active attachment. Attachment-specific
checks still need the backend's guarded enrollment state. Imported services may
rewrite replies to broker-owned `_R_` subjects; those are rejected here. General
operator/import response handling remains unsupported until its containment is
verified against the broker.

The broker and callout rules come from
[ADR 0081](../../../../docs/adr/0081-nats-tls-by-default-and-operator-mode-admission.md)
and [ADR 0082](../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md).
The service/method suffix follows
[ADR 0016](../../../../docs/adr/0016-protobuf-rpc-over-nats-micro-binding.md).


## Request transport

`PeerRequester` accepts an already connected client and fixed typed account,
caller, connection and timeout settings. Calls accept a method and payload, with
an optional header map for the shared RPC codecs. It derives the route, builds a fresh exact reply subject, and validates
that reply with `PeerReplySubject` before sending. An explicit SDK request inbox
preserves this shape without using the default request multiplexer.

The positive timeout is capped at two seconds per RPC. An outer deadline covers
the whole send-request future, including subscription and publish queueing; the
SDK's response timeout is bounded too. Both timeout paths retain the typed
transport timeout classification. A timeout does not establish that remote
effects were absent.

This per-RPC bound is separate from the host hook's two-second total budget.
An adapter that performs enrollment and batch preparation needs an end-to-end
hook deadline across those calls. The requester does not add authentication or
prove that the broker applied the supplied permission profile.

## RPC binding and composition

`serve_peer_rpc` registers the annotated `PeerService` through `grpc-nats-micro`.
Discovery preserves its version, description, RPC names and default queue group.
The service annotation restricts requests to protobuf. The shared binding also
supports canonical JSON, including readable JSON policy-rejection statuses.

The shared metadata admission seam receives the delivered subject and reply
before negotiation, decoding, handler execution or either response channel.
`PeerRpcAdmission` validates those values through `PeerIngress` and
`PeerReplySubject`. Missing or unsafe routing drops the request without a reply.
Headers cannot establish the owner. The admitted handler derives
`TrustedPeerOwner` from the subject principal and dispatches a typed
`PeerRpcOperation<Request>`.

`PeerRpcApplication` composes the existing `PeerService` and `PeerApplication`.
Their command runtime maps the connector caller to an `Agent` principal and
requires both application and core command authorization. Store, policy, clock,
content, prepared presentations, IDs and catalog remain mandatory injections.
Reconciliation stays an explicit bounded application operation, outside the RPC
inventory. Application outcomes such as `RETRY_UNAVAILABLE` remain successful
response bodies. Typed failures map to fixed canonical statuses without exposing
request text, references, fingerprints or backend diagnostics.

`PeerRpcClient` supplies typed methods for the annotated RPCs over `PeerRequester`.
It encodes requests with `Content-Type` and uses the shared reply decoder, which
honors the reply's encoding and treats `Nats-Service-Error-Code` as authoritative.
Complete `google.rpc.Status` details reach the caller. It performs no automatic
retry and never changes an idempotency key after a timeout.

## Durable peer histories

`peer::open_store` takes an existing JetStream context and a tenant-specific
`PeerStoreConfig`. It composes the shared decider `JetStreamStore`, including
its atomic append and expected-position checks. `PeerJetStreamStore::open` wraps
that validated open path for the service's `ScopedPeerStore` contract. Its tenant
comes from the actual immutable subject resolver; callers cannot attach an
unrelated scope label or bypass resource validation through its constructor.
Event subjects are
`{tenant}.peer.v1.peer.{peer_id}.events`; only the service accesses them.

A positive finite `PeerStorageBudget` bounds the file-backed event stream.
Limits retention, discard-new behavior, and disabled age/count eviction preserve
accepted history when capacity is exhausted. Full storage rejects writes.
A separately bounded snapshot KV bucket retains one revision per key. Snapshots
are optional runtime accelerators; they do not replace authoritative history.

The adapter does not enforce a replica count. New event streams and snapshot
buckets use JetStream client and broker defaults; existing replica counts are
neither changed nor validated by `open_store`. File storage and retention checks
do not establish node-failure tolerance. Deployments requiring that guarantee
must provision and verify replication separately.

Opening an existing resource checks its storage, subject scope, capacity and
retention settings and fails on unsafe configuration. It does not reconfigure
that resource. Initial provisioning uses the shared get-before-create helpers;
use one trusted provisioner until concurrent first creation with divergent
settings has its own exclusion contract.

## Broker evidence and limits

Testcontainers fixtures exercise atomic replay after reopening, stale and
concurrent writers, tenant separation, full-store rejection and unsafe-resource
refusal. A TLS fixture generates disposable certificates and nkey credentials.
Its explicit shared account applies distinct connection grants and checks actual
broker permission errors for impersonation, other connections' reply inboxes,
raw stream and KV access, consumer creation/update, pull and direct-get. Each
protected API has a privileged positive control. This is not an operator/import
topology proof.

The application exchange uses the actual binding, typed client and JetStream
store. Content, presentation, catalog and policy adapters are disposable fixtures
exposed only by `test-support`. They do not implement ADR 0083 sealing or durable
production cache retention. The exchange checks durable send, explicit trusted
reconciliation, lost-reply replay through reconstruction, application
acknowledgment and separate sender settlement/status. Scope and recipient
capacity failures remain closed. These tests do not prove native-host insertion,
model comprehension, production readiness or long-lived checkpoint recovery.

Run the focused broker suite serially when container startup capacity is limited:

```sh
MISE_EXEC_AUTO_INSTALL=false mise exec -- cargo test -p trogon-peer-nats -- --test-threads=1
```
