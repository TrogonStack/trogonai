# Peer NATS routing and storage

This crate defines peer request subjects, connector permissions, and the durable
peer event-store adapter.
It does not connect to a broker, mint credentials, authorize collaboration
membership, or implement the peer service.

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


## Durable peer histories

`peer::open_store` takes an existing JetStream context and a tenant-specific
`PeerStoreConfig`. It composes the shared decider `JetStreamStore`, including
its atomic append and expected-position checks. Event subjects are
`{tenant}.peer.v1.peer.{peer_id}.events`; only the service accesses them.

A positive finite `PeerStorageBudget` bounds the file-backed event stream.
Limits retention, discard-new behavior, and disabled age/count eviction preserve
accepted history when capacity is exhausted. Full storage rejects writes.
A separately bounded snapshot KV bucket retains one revision per key. Snapshots
are optional runtime accelerators; they do not replace authoritative history.

Opening an existing resource checks its storage, subject scope, capacity and
retention settings and fails on unsafe configuration. It does not reconfigure
that resource. Initial provisioning uses the shared get-before-create helpers;
use one trusted provisioner until concurrent first creation with divergent
settings has its own exclusion contract.

The tests exercise atomic replay after reopening, stale and concurrent writers,
tenant separation, full-store rejection, and refusal of an existing evicting
stream. They do not establish broker authorization, sealed content storage, or
native-host delivery.
