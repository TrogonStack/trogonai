# Integrate workload identity into a trusted host

Use the existing `trogon-workload-identity` consumer when a trusted service
needs rotating X.509 workload credentials. The host owns the Workload API
connection and keys; guest code does not receive the socket or private material.
The [crate reference](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/platform/trogon-workload-identity/README.md)
describes the Rust API and wire bounds. The
[architecture explanation](../architecture/workload-identity.md) distinguishes
transport identity from application identity and authorization.

This procedure describes implemented consumer behavior. Qualified attestation,
issuer custody, socket isolation, provider federation and workflow admission
remain separate integration or deployment work. Selecting a new protobuf
execution profile does not implement those owners.

## Connect the protected inputs

Configure the exact local Unix endpoint and expected SPIFFE ID. Restrict access
to the trusted host and isolate its key material from other workloads and
tenants. A read-only socket-directory mount still permits connecting to the
socket; mount permissions alone do not establish caller isolation.

Create `IdentityStore` with an `IdentityConfig` and an `ActivationAudit`
implementation. Run the returned `IdentityDriver` alongside the owning service
and its shutdown lifecycle. Supply complete validated `RegistrationSet`
replacements from the authenticated authority reader. The egress integration
already implements that reader; another service must not substitute a caller
header, a static SPIFFE-shaped alias or a self-renewed registration.

A registration binds scope, recovery fence, registration/trust revisions,
finite expiry, exact mappings and approved root fingerprints. A valid
`RegistrationSet` value still needs current trusted provenance. The runtime
must validate its wire input and authority source rather than assume inert
protobuf annotations performed admission.

The audit implementation acknowledges actual durable activation storage. A
queued write, local log or constructed receipt is insufficient. The consumer
retains the latest bounded candidate while audit is pending and performs a
final source/registration/trust comparison before activation. The stored event
describes intent; its acknowledgement does not prove that the final comparison
enabled the material.

## Retain a lease through each operation

Obtain a `MaterialLease` from the store only when material is available. A
missing lease means the operation cannot use this workload profile. Do not fall
back to a file credential, alternate identity or earlier cached permit.

Use the lease's `server_config` or `client_config` for the handshake. Client
configuration requires the exact expected peer SPIFFE ID; its DNS server-name
argument is not the SPIFFE identity policy. The current server profile admits
registered agent bindings within its configured scope and trust domain.

Retain the lease through handshake and established-session work. The owner must
react to all of these boundaries:

| Boundary | Required action |
| --- | --- |
| `is_current()` refuses | Stop the admitted operation and close affected local I/O. |
| `changed()` completes | Recheck authority and withdraw the captured generation; do not silently transfer a live session to new material. |
| `deadline()` arrives | End the operation even if wall time moved backward or the source remains connected. |
| Independent destination or operation policy changes | Apply that policy's withdrawal and deadline independently of credential currentness. |
| Service shutdown | Stop the drivers and owned connections without leaving detached work using stale authority. |

TLS resolvers, signers and verifier callbacks check current material. Those
callbacks protect new cryptographic work; they do not close a socket that
already completed its handshake. The host must select the lease notifications
and deadlines alongside DNS, connection, audit and forwarding waits. Retaining
an `Arc` to a TLS configuration cannot extend a lease. Session tickets and
resumption caches are disabled by the supported profile.

## Preserve authenticated peer evidence

Call `peer_evidence_from_certificates` with the peer chain authenticated by the
actual handshake. Do not use certificate bytes from a header or a requester
field. The evidence carries the exact registered application identity, SPIFFE
identity, leaf fingerprint, selected-path expiry and monotonic deadline.

Bound the session by the earliest local lease, peer-path deadline and operation
policy deadline. Carry the peer's original monotonic deadline into the session;
recomputing it from a later wall-clock sample can extend authority after a
clock rollback. Keep UTC expiry for audit without using it to restart the
already observed lifetime.

The governed registration supplies the application mapping. A URI path or
caller-selected application ID cannot grant that mapping. A valid peer
certificate still needs current operation authorization and destination policy.

## Handle withdrawal and recovery

Treat complete omission of bindings or approved roots as valid withdrawal.
Retain revision floors and refuse equal or older authority after withdrawal.
Source disconnect or permission denial withdraws usable material without
waiting for audit, and recovery requires newer governed registration.

The supported raw wire decoder refuses CRL-bearing responses because complete
CRL processing is not implemented. Do not strip CRLs before the consumer or
describe unsupported-profile refusal as revocation verification. Qualification
of a CRL-dependent issuer requires signature, scope, freshness and path-aware
revocation support first.

Local tests can use the crate's `test-support` Unix Workload API and TLS
fixtures. Acceptance should include audit held during withdrawal, source loss,
root removal, registration replacement, cached TLS reuse and an already open
connection. Keep positive controls so setup failure cannot masquerade as the
security result. Real issuer, attestor, HSM, HA and independently fenced restore
proofs remain required by the
[runtime reference](../reference/workload-identity.md).

For the proposed workflow version/profile and run-admission join, use the
[configuration contract reference](../reference/workload-identity-configuration.md).
The existing consumer can contribute current identity evidence; it does not
authorize a workflow or mint provider credentials by itself.
