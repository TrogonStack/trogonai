# Supervisor workload identity

This crate consumes X.509 credentials from an explicit Unix Workload API socket
and activates them under current governed registration and trust. The trusted
host owns the socket client and private keys. The application identity remains
the exact registered value, independently of the SPIFFE transport identity.

`IdentityStore::new` returns a registration controller and an `IdentityDriver`.
The host runs the driver alongside its service and supplies complete
`RegistrationSet` replacements from an authenticated authoritative projection.
Each set binds the configured recovery fence, monotonic registration and trust
revisions, a finite expiry, exact workload bindings and approved root certificate
SHA-256 fingerprints. Registration, source observations and active material share
one activation lock. Empty binding or approved-root sets represent complete
revocation and retain their registration and trust revision floors. Neither
withdrawn registration nor withdrawn trust can be restored by replaying an
older or equivalent generation.

The driver selects the configured SPIFFE ID, validates its certificate profile,
selected certificate paths and matching private key, and checks its bundle against
the exact approved root set. A certificate validity window may not exceed one
hour. A registration lease may not exceed 330 seconds. A monotonic deadline also
prevents a backward wall-clock adjustment from extending admitted authority.

New material requires `ActivationAudit::record` to acknowledge durable storage
before a final source, registration and trust comparison enables it atomically.
The event describes an activation intent. Its acknowledgement does not prove that
a later comparison allowed activation. Source ingestion continues while audit is
pending, and retains a bounded latest candidate. Omitted identity, removed roots,
permission denial and disconnected source withdraw usable material without waiting
for audit. Reconnection after withdrawal requires newer governed registration.

`MaterialLease` captures one approved generation. Its server and client TLS
configurations check live authority in certificate resolvers, signing operations
and peer verification callbacks. Retaining a configuration does not extend a
withdrawn or expired credential. Session caches and tickets are disabled.
Client verification authenticates the exact registered SPIFFE URI, independently
of the TLS API's DNS server-name argument. Server verification admits registered
agent bindings in the configured tenant, project and trust domain.

The caller must retain the lease through each handshake and established session,
check `is_current`, and select `changed` and `deadline` alongside network work.
TLS configuration callbacks protect new handshakes; the caller closes connections
that already exist. `peer_evidence_from_certificates` takes the chain authenticated
by that handshake and returns the exact transport identity, public leaf fingerprint,
registered application identity, selected peer path expiry and a monotonic deadline
anchored before path validation. The caller must carry that deadline into its
session bound; a later wall-clock sample can extend the path lifetime after a
rollback. The actual path expiry remains available for audit evidence. The peer
path may expire before the local credential and must also bound the session.

Activation fingerprints use SHA-256 of leaf certificate DER, leaf
SubjectPublicKeyInfo DER, and the concatenation of sorted root DER SHA-256 hashes.
The fixed-width root hashes make the bundle fingerprint independent of source
ordering. Audit facts contain public identifiers and fingerprints, never keys.

The client limits decoded gRPC responses to 1 MiB before SDK domain parsing. The
platform additionally bounds credential and bundle bytes to 1 MiB, SVID entries
to 32, bundle entries to 16, SPIFFE URIs to 2048 octets, roots per bundle to 16,
and registration bindings to 256. Duplicate SPIFFE IDs and root fingerprints are
refused. Each reported identity must match its
certificate. Bundles for the same SVID trust domain must contain the same roots,
independently of order. A federated entry cannot replace an SVID domain's bundle.
No implicit trust-domain federation is enabled.

The SDK's parsed X.509 context does not preserve Workload API CRLs. The client
therefore decodes the official wire response before SDK SVID and bundle parsing.
Any supplied CRL withdraws authority and prevents activation. This profile uses
governed identity and root withdrawal with bounded credential validity. A
deployment requiring CRLs needs complete CRL verification before enablement.
Owned wire private-key buffers are redacted in debug output and zeroized on drop.

The wire schema and `/SpiffeWorkloadAPI/FetchX509SVID` RPC path follow the
[SPIFFE Workload API](https://github.com/spiffe/spiffe/blob/main/standards/SPIFFE_Workload_API.md#5-x509-svid-profile)
and the [pinned SDK schema](https://github.com/maxlambrecht/rust-spiffe/blob/ac70f979975939773ecc79d9763b2dba07d57746/spiffe/src/proto/workload.proto).
The response uses tags 1 for SVIDs, 2 for CRLs and 3 for federated bundles. The
SDK supplies the Unix connector and SVID and bundle parsing; this crate owns
bounded raw gRPC decoding and the activation profile checks.

Issuance, node and workload attestation, constrained SPIRE registration, signing-key
custody, socket isolation, HA and recovery belong to the deployment. The real Unix
protocol and TLS fixtures under `test-support` verify this consumer, not those
production boundaries. The [workload identity profile](../../../../docs/reference/workload-identity.md)
and [draft ADR0098](../../../../docs/adr/0098-attested-workload-identity.md)
retain their separate acceptance requirements.
