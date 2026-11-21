# Peer NATS routing

This crate defines the peer request subjects and connector permission profile.
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
