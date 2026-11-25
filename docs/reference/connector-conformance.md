# Connector Conformance

This reference defines repeatable contract artifacts and release acceptance
requirements. Validating a fixture establishes its stated property, not a
running provider integration, authorization boundary or deployed service.

## Contract artifacts

| Artifact | Property |
| --- | --- |
| [Account identity vectors](/connector-fixtures/connector-account-identity-v1.json) | Exact framing, full HMAC-SHA256 and canonical claim identity; required rotation scenarios |
| [Relay signature vectors](/connector-fixtures/connector-relay-signatures-v1.json) | Public-key HTTP signatures, content integrity, tampering and proof-expiry cases |
| [Recovery traces](/connector-fixtures/connector-relay-recovery-v1.json) | Claim/audit/send/outbox ordering and forbidden recovery transitions |
| [Linear definition](/connector-fixtures/linear-connector-definition-v1.json) | Reviewed first-provider metadata compatible with the current protobuf image |
| [Linear response policy](/connector-fixtures/linear-response-policy-v1.json) | Exact declarative response/error policy pinned by SHA256 in that definition |

Signature fixtures contain only public keys, signatures and synthetic request
data. Test private keys are generated transiently and never persisted. The
reference verifier exercises artifact cryptography and a reference recovery
model; a production relay must be tested independently against the same inputs.

Run the checked-in verifier from the repository root:

```sh
mise exec -- python3 scripts/verify-connector-conformance.py
```

See its dependency/usage description before running in a different environment.
Compile/lint all protobufs with Buf and validate the candidate definition against
the compiled `trogonai.connections.catalog.v1alpha1.ConnectorDefinition`. A
provider profile also needs semantic checks: static read-only identity query,
declared field types, endpoint/role membership and exact response-policy digest.
Syntax/CEL compilation alone does not prove those implementation behaviors.

## HTTP proof interoperability

Use Ed25519 over the RFC9421 signature base directly, not an extra hash passed
to an Ed25519ph API. The confirmation keyid is the unpadded base64url RFC7638
SHA256 thumbprint of the public OKP JWK, using RFC8037's `crv`, `kty`, `x` members.
The verifier obtains that key from attested service state. A key URL or embedded
caller public key is never a trust source.

Sign and verify the exact context header bytes, including any semantically
equivalent protobuf encoding. Unknown fields, duplicate singular fields and
invalid wire messages are rejected by preflight; there is no general canonical
protobuf byte representation. A changed context needs a new valid signature.
Header component coverage and canonicalization follow the published relay
profile and RFC9421, including the target/query and identity Accept-Encoding.
[HTTP Message Signatures](https://httpwg.org/specs/rfc9421.html),
[RFC7638](https://www.rfc-editor.org/rfc/rfc7638.html) and
[RFC8037](https://www.rfc-editor.org/rfc/rfc8037.html) define these building blocks.

Release tests must include wrong keys/algorithm, missing/extra unsigned headers,
method/path/query/context/body mutation, invalid digests, duplicate field lines,
future/expired proofs, stale grants, revoked keys and concurrent replay at
different replicas. A valid signature never substitutes for policy or custody
authorization.

## Workload identity acceptance

Authenticate a worker successfully, then verify custody still refuses missing
caller-role registration, a different tenant, mismatched binding or audience,
and an absent, expired or revoked grant. These are independent authorization
checks under [ADR#0095](../adr/0095-connection-credential-custody-and-refresh.md).

The proposed production [workload identity profile](./workload-identity.md)
requires the following gate before enablement. Exercise the actual attestor and
issuer at bootstrap, issuance and renewal:

- A copied signing key in an unattested runtime cannot obtain credentials.
- Evidence for another workload, runtime or public key cannot authorize issuance.
- Expired or replayed attestation evidence is refused.
- An unavailable attestor or failed renewal cannot bypass attestation or silently
  select static credentials.

A URI, certificate fixture or proof-of-possession test does not establish these
properties. A compatibility bootstrap does not satisfy this gate. The
[workload identity checklists](./workload-identity.md) additionally cover
issuer isolation, key/bundle rotation, deferred issuance, token cache identity,
sender constraints and established-session revocation. Specifying these gates
does not implement or close the attestation work deferred by
[ADR#0053](../adr/0053-external-oidc-federation-surface.md).

## Recovery and isolation acceptance

Run the [recovery matrix](./connector-relay-recovery.md) against the real journal,
audit stream, gateway and network adapter, including process crashes and lost
acknowledgements at every phase. Required evidence includes:

- One authoritative nonce winner across replicas; no permission granted by a
  stale absence read or expired lock.
- No provider send before durable acceptance and send intent; no resend after
  uncertain send, response rejection or lost reply.
- Durable completion recovery while audit publication is unavailable, with
  capacity reservations and count/age/byte gates enforced.
- Renewal that cannot shorten replay retention, pruning that cannot resurrect
  an invocation and restore fencing that invalidates old grant/key authority.
- Parent/caller isolation in routes, storage, list cursors, status receipts,
  resource names and all authoritative foreign-reference checks.
- Exact-IP destination dialing with hostname/SNI/certificate verification,
  rejection of forbidden networks and validation on every connection fallback.
- Bodyless/framing/content-coding/range/SSE/upgrade rejection and no partial
  release of a response containing a detected injected credential.

## Identity and onboarding acceptance

Exercise concurrent allocation/hold, same-account reconnect, mismatched account,
wrong attempt/stage/xkey, expired observations, raw identity in prohibited outputs,
and missing/unsupported MCP probes. Rotation tests must race the actual claim
append with cutover, interrupt backfill, lose aliases/keys, restore old snapshots
and erase profile revisions. Incomplete coverage refuses allocation; it never
creates a second active claim or silently extends key authority.

Product API tests cover CSRF/origin checks, same-session callback, fixed return
destination, unknown/unauthorized equivalence, revision/generation conflicts,
idempotency collisions/expiry, lost redirect, cancellation during exchange,
disconnect cleanup and response/log redaction. These use the
[control contract](./connector-control-api.md), not public aggregate commands.

## Provider enablement receipt

The first provider needs the real app registration, exact callback, release and
response-policy digests, deployed build, minimal scopes, token lifecycle and
failure/cleanup proof. Recheck current provider documentation and limits, record
typed evidence and keep material opaque. A fixture cannot stand in for OAuth
consent, provider approval, refresh/revoke success or workload isolation.
