---
number: "0066"
slug: secret-service-and-connection-boundaries
status: accepted
date: 2026-09-24
---

# ADR#0066: Secret Custody and Connection Authority Have Separate Contracts

## Context

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) selects
OpenBao behind a platform secrets service. The credential prototype proves a
write saga but still places custody inside the gateway. Meanwhile,
[ADR#0063](./0063-agent-connection-declarations.md) gives an Agent a way to
declare external dependencies without defining their resources or authority.
Implementing either boundary independently would leave every adapter to decide
how a stored credential becomes usable.

This record settles the contracts for material admission, custody, authorized
use, and connector connections. [ADR#0067](./0067-webhook-ingress-and-delivery.md)
applies them to receiving and sending webhooks.
[ADR#0068](./0068-provisioning-sagas-and-owned-resource-cleanup.md) fixes
provisioning, ownership transfer and compensation. Only durable event protobufs
and their value types are delivered; transport contracts, service extraction
and Rust implementation are separate work.
This decision was accepted following maintainer signoff under
[ADR#0000](./0000-adr-process.md).

## Decision

### Ownership and persistence

The platform secrets service is the sole OpenBao client. It owns vault and
credential metadata, material writes, rotation, destruction, recovery,
authorization of secret use, and bounded material caches. The gateway owns
provider ingress. A connection executor owns outbound provider calls and MCP
connections. A webhook delivery worker owns platform event delivery. Channel
bridges retain conversation and reply semantics; credential-bearing transport
uses the connection executor. These are responsibility boundaries, not a
requirement to introduce a new deployment for every adapter.

All resources belong to an immutable project, following
[ADR#0046](./0046-project-anchored-resource-hierarchy.md). Public resource names
are rooted at `projects/{project}`. Vaults are independent metadata aggregates
with their own identity, display label, environment, policy and lifecycle.
They group credentials; they do not create an OpenBao namespace or another
tenant boundary. Vault and credential display labels may change through
metadata events. Their environment and purpose are immutable policy identity;
changing either requires replacement rather than a relabeling that changes
authority. Cross-project moves are prohibited. Vault removal fails while
live credentials remain; disabling a vault prevents new uses of its children.

[ADR#0047](./0047-event-sourced-credential-metadata.md) applies to the extracted
service unchanged: JetStream metadata events are authoritative, NATS KV holds
protobuf idempotency and operation records, and read models are projections.
No relational write model or second credential lifecycle is introduced.
Connection configuration and binding activation are governed by the connection
aggregate. Replacing a role binding is atomic in that aggregate, so concurrent
commands cannot install competing active bindings for the same role.

### Stable identity, physical versions, and lifecycle

A `SecretRef` is a server-minted opaque identifier: 32 cryptographically random
bytes encoded as unpadded base64url. Its identity carries no provider, project,
vault, path or material version. The service associates the identifier with its
project and vault and verifies that association against authenticated context.
Knowing a ref confers no authority. Identifiers are never reused.

Rotation preserves the ref. Explicit version references are metadata for
auditing and recovery, not permission to read an old version. The service's
private adapter maps the identity and version to OpenBao. The prototype's
parseable internal credential IDs are not a public wire format; extraction
requires a migration map and no renaming of already stored event types in place.

Credential policy state and physical material state are different facts.
Disabled or revoked credentials are unusable even if the backend still holds
their bytes. A pending write is not active merely because OpenBao has a newer
version. Activation requires the correlated successful write and its metadata
event. Physical deletion and destruction are confirmed from backend metadata.
Revocation denies new uses first; provider revocation and physical destruction
are retryable cleanup, never prerequisites for the local denial.

### Material enters through an ephemeral exchange

Secret input travels directly from an authenticated management caller to the
secrets service. Provider OAuth callbacks terminate in a trusted platform
component and submit acquired tokens through the same boundary. They never
place authorization codes, tokens, verifier inputs or request bodies in a
durable command envelope. A browser may submit a credential; it cannot read
stored material through the metadata API.

The future exchange reserves the credential stream, fixes a metadata-only
provisioning plan before side effects, writes material with a backend
compare-and-set precondition, and records verified activation.
[ADR#0068](./0068-provisioning-sagas-and-owned-resource-cleanup.md) defines
unknown outcomes, ownership, write fences and compensation through confirmed
absence. Failed setup cannot terminate while it still owes cleanup.
Idempotency is scoped to authenticated principal, project, operation kind and key. Secret values are excluded from the stored request
fingerprint. Reusing a key resumes the same operation and cannot replace its
material. An authorized resubmission names the same pending operation and
requires settled absence; a fresh transport request identity cannot change the
external effect key or overwrite a candidate already observed.

A crash before the material write requires resubmission. A crash after the
write is reconciled from operation attribution atomically stored beside the
material inside the private KV v2 data envelope. Path-scoped custom metadata
cannot establish version ownership. The worker never
recovers material from a stream, response cache or retry queue. Concurrent
rotations are serialized by aggregate revision and backend version
preconditions. The loser cannot activate or overwrite the winning version.

The one-time rule in [ADR#0048](./0048-one-time-plaintext-exposure.md) remains:
only the winning direct generation response can return a newly generated
shared webhook secret. Imports are not echoed. Replayed responses, operations
and reads contain metadata only. Losing a generated value requires rotation;
there is no escrow or recovery endpoint.

Material-bearing transport schemas are deliberately outside this event-only
change. Material is forbidden in DeciderService durable requests, JetStream
streams, snapshots, idempotency values, telemetry and error details. Transient NATS core
request/reply subjects must be outside all stream capture patterns. Encryption
in transit and workload authentication apply to these exchanges. Generated
types alone do not establish that separation.

### Material leaves custody only for an authorized operation

The secrets service exposes value resolution, shared-secret MAC operations and
constant-time equality-token verification.
MAC computation and verification allow the gateway or delivery worker to use a
provider shared secret without retrieving it. These operate on retrievable
SecretStore material; they are not an invented Transit import mechanism.
Non-exportable keys and cryptographic key operations retain the separate
`KeyManagement` and `KeyRef` boundary of
[ADR#0023](./0023-secret-management-and-key-custody-direction.md).

Value resolution is restricted to trusted platform adapters whose protocol
requires it, such as an upstream Authorization header or a WebSocket login.
Agents, tools running in an agent's address space, prompts, user configuration,
and metadata clients have no resolve authority. A connection executor supplies
credentials at the authenticated transport boundary and rejects caller-supplied
authentication headers or attempts to override the destination.

Every use is backed by a server-authorized context naming the project, exact
ref, binding revision, purpose, workload identity, operation, destination
policy, audience and expiry. The context is metadata, not a bearer credential.
The receiving service compares it with transport-authenticated identity and
live authorization. Caller-supplied IDs, headers and project fields are lookup
inputs, never identity evidence. An expired, revoked, mismatched or unavailable
authorization fails closed before material is returned or a MAC is computed.

Purpose distinguishes provider authentication, OAuth refresh, tool and MCP
authentication, webhook verification, webhook signing and secret webhook
destinations. Short-lived agent-workload leases record issuance, renewal and
revocation without exposing provider lease identifiers or credential values;
provider-side issuance composes the same provisioning protocol. Incoming
verification cannot authorize outgoing signing or provider API calls. A
credential may serve only its declared purpose; a different purpose requires
a separately admitted credential and binding. No arbitrary header template or
secret interpolation language is exposed to callers.

### Cache and revocation rules

Material caches live only inside the secrets service. Consumers own resolved
bytes for the current bounded operation and release them immediately afterward.
Rust implementations must use single-owner, zeroizing values with redacted
formatting and must not derive secret-bearing debug, serialization or cloning
behavior by default. This is an implementation requirement, not a claim about
the current prototype.

The cache and authorization freshness deadline is absolute and at most 330
seconds from the authoritative observation. A cache hit, another projection,
or another service hop cannot restart it. Fan-out invalidation reaches every
replica. The p99 five-second objective and missed-event ceiling in
[ADR#0049](./0049-revocation-latency-target.md) remain in force. A replica whose
authoritative state cannot be refreshed by the deadline stops serving uses.

Live connections are not an exception: their supervising adapter must recheck
authorization and close or reauthenticate within that deadline. A token is
not retained merely because a socket is long-lived. Revocation cannot retract
bytes already sent or undo a provider side effect; external revocation is a
separate recorded operation.

Only the current version authenticates new provider calls and signs new
outgoing messages by default. Webhook verification may admit a previous
version until an explicit overlap deadline, bounded by 24 hours. Outgoing
dual-signing, when requested, is separately authorized for that overlap.
Disabled, revoked or destroyed material is never an overlap candidate.
Compromise revokes the old version without grace. Provider account continuity
is enforced for account-bearing provider-authentication roles; accountless
adapters validate endpoint identity when their contract requires a probe.
Generated webhook keys require local type and entropy checks, without inventing
an external account. `WebhookRotationOverlapEnded` records immediate termination of the previous
key's window while preserving the current key and creating a retirement obligation. Provider account continuity
must be verified before a rotation is activated; a changed account requires a
replacement connection.

### Connector, connection, binding and grant

A connector is provider-specific implementation knowledge. `ConnectorVersion`
pins an immutable adapter release, its artifact digest, supported operation
handles and required authentication roles. The provider is an open identifier,
not a platform-wide enum. Registering an operation requires a documented input
contract, destination policy, authentication behavior and secret-free output
contract. Unknown versions, operations and authentication modes reject.

A connection is a project-owned external account and security boundary. It
pins a connector version, a validated non-secret endpoint/account identity and
configuration digest. Labels and display names are mutable metadata. Changing
the account or public endpoint creates a replacement. An explicit adapter
upgrade creates a new immutable configuration snapshot only after validation
proves identity continuity. Existing admissions never silently acquire it.

Configuration is serialized once after validation. The authoritative record
retains those exact bytes and their SHA-256 digest. Readers verify the stored
bytes before decoding; they never recreate the digest by re-encoding a parsed
protobuf message. Material versions, display labels and grants are outside
those bytes.

Enrollment starts pending. A staged binding permits a narrowly authorized
management probe against the admitted destination before ordinary connection
grants can exist. Only a trusted verifier may record successful account and
capability verification and activate the connection. Failed OAuth refresh puts
it into a reconnect-required state. Disabling is reversible suspension;
reenabling repeats readiness validation and never revives revoked grants.
Revocation is terminal.

Provider-authentication rotation stages a candidate version while the previous
active version continues serving. A management verification context may read
only that pending operation's candidate for a bounded provider probe. A trusted
verification result bound to the candidate, account and connection configuration
must exist before activation. Recovery after a successful backend write waits
for that proof; backend metadata alone does not prove account continuity.

A credential binding connects a named connector role to a stable SecretRef.
Changing the role, purpose or credential creates a replacement binding;
rotating the material under the ref does not. An unauthenticated operation is
an explicit supported adapter capability, not the meaning of a missing value.
OAuth refresh stays in the trusted executor/secrets boundary, with a serialized
refresh operation and recoverable material-write saga. Failed refresh marks the
connection as requiring reconnection and never selects another account.

Agent declarations remain provider-plus-label requirements without credential,
resource or version pins. Admission rejects ambiguous matches even for optional
requirements. It records the selected connection configuration and adapter
digest, then separately authorizes the requested operations. This refines the
reference-only admission suggestion in the earlier research proposal: a digest
over non-secret configuration cannot be invalidated by material rotation, and
prevents an endpoint or adapter change from altering an admitted execution.

A connection grant is limited to a Session execution attempt or an authenticated
workload operation, permitted operations, binding roles, audience, confirmation
key and an expiry no more than five minutes after issuance. It is held by the
platform supervisor, not by the agent implementation. Each use still checks
live revocation and policy. New attempts, child Sessions and delegated work
must obtain their own grants. Service-driven ingress and outgoing webhook
delivery use workload-operation authorization; they do not invent a Session
merely to use a credential. Model routing remains owned by
[ADR#0032](./0032-model-route-and-credential-binding.md); these contracts do not
replace its model selection, billing or route records.

### Network and audit boundaries

Endpoint admission validates scheme, origin, TLS and resolved addresses before
credential resolution. Each new connection validates the current DNS answer
and connects to the validated address. Redirects cannot move credentials or
payloads to another origin. Private, loopback, link-local and metadata-service
destinations are denied by default. Self-hosted private integrations require
deployment-attested network policy; a project caller cannot create that trust
by submitting a CIDR or URL. A token-bearing path or query belongs in
SecretStore, with a separately validated public origin and purpose-bound
destination binding.

Successful secret use requires a durable metadata audit acceptance before
material release or MAC computation. Audit failure fails the use closed,
including cache hits. Lifecycle commands persist their correlated audit fact
with the owning metadata event. Denials emit bounded non-secret reason codes;
audit failure on a denial cannot turn it into success. Audit stores caller,
project, binding, ref, purpose, operation and outcome, never raw values,
upstream authentication headers, callback URLs or provider response bodies.
OpenBao's provider audit remains enabled and correlated with this business audit.

Failure categories are closed protobuf enums. Provider error text cannot become
a reason code or public error detail. Missing resources and denied access share
the externally indistinguishable denial path where revealing existence would
cross an authorization boundary. Anomaly detection consumes these metadata
facts: cross-project attempts, denied purpose changes, replayed authorization
and unexpected resolve rates per workload. Denial is inline; alerts and
deployment-specific thresholds cannot grant authority or silently change
credential policy. Disabling a resource after an alert remains an explicit
audited control-plane action.

### Deployment and scope

Hosted and self-hosted deployments use the same resource and protobuf model.
Deployment-attested OpenBao login, minting-only bootstrap policy, unseal,
backup and break-glass remain governed by
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) and
[ADR#0052](./0052-cloud-kms-production-seal.md). Application secrets never
bootstrap the service that must read them. Production readiness requires
separate evidence in each deployment model.

Platform-issued caller API keys remain verifier-only under the existing API-key
decisions. Password hashing, host bootstrap material, business-data envelope
encryption and model-route admission are not redefined by this contract. New
protocols may extend the connector catalog without weakening these boundaries.

## Consequences

- The extraction questions have explicit answers: value and MAC operations,
  service-owned caches, event-sourced lifecycle, and stable opaque refs.
- A ref, a connection declaration and a usable grant have distinct types and
  authority. Owning any one does not imply possession of the others.
- The prototype's gateway cache and direct OpenBao access require migration.
  They are not grandfathered into the target architecture.
- Secret use adds authorization and audit dependencies. Their failure stops
  credential-bearing work instead of creating an unrecorded fallback.
- The contracts remain excluded from Rust code generation. Per
  [ADR#0064](./0064-schema-constraints-as-documentation.md), validation
  annotations document and type-check invariants but do not enforce them.

## References

- [Secret and connection contract reference](../reference/secret-and-connection-contracts.md)
- [OpenBao KV v2 documentation](https://openbao.org/docs/secrets/kv/kv-v2/)
