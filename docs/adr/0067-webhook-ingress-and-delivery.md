---
number: "0067"
slug: webhook-ingress-and-delivery
status: accepted
date: 2026-09-24
---

# ADR#0067: Incoming Webhooks and Outgoing Deliveries Have Independent Authority

## Context

The gateway receives provider events before any Session exists. Outgoing
platform events may outlive the Session that produced them. Neither path can
borrow an Agent's grant or infer authority from possession of a callback URL.
The existing [channel routing](../architecture/multi-channel-agent-routing.md)
also gives the gateway an inbound role; outgoing delivery needs an explicit
owner rather than an accidental expansion of that process.

This record applies [ADR#0066](./0066-secret-service-and-connection-boundaries.md)
to both directions and fixes durable event contracts before runtime work.
[ADR#0068](./0068-provisioning-sagas-and-owned-resource-cleanup.md) supplies
shared provisioning and cleanup facts; no transport or command schema is part
of this deliverable. This decision was accepted following maintainer signoff
under [ADR#0000](./0000-adr-process.md).

## Decision

### Resources and ownership

An inbound endpoint belongs to a project and connection. Its server-minted
route identifies a configured verifier operation, verification binding,
accepted event types and replay policy. The route is a locator, never a secret
or a grant. An outbound subscription belongs to a project and connection and
selects explicitly exposed platform event types, a destination and its signing
binding. Receiving from a provider grants no permission to call it. Sending
events grants no ability to verify or ingest events from that recipient.

The ingress gateway verifies incoming requests. A delivery worker consumes
authorized platform events and sends outgoing deliveries. Both authenticate as
platform workloads and acquire purpose-bound use authorization. Connection,
endpoint/subscription, binding, credential and workload policy must all permit
the operation. Resources can be paused for reversible suspension or revoked
terminally. Updates use aggregate revision preconditions; identity, direction
and security boundary cannot be patched into a different resource.

Paused subscriptions retain queued deliveries until their deadlines but admit
no new attempts. Revocation cancels pending delivery authority. Resuming a
paused resource repeats current authorization; it does not bypass expiry.

### Provisioning and removal

Reserve the endpoint or subscription stream in provisioning mode before any
provider registration, generated signing credential or child resource is
created. The shared plan records exact ownership and recovery capability.
Activation adopts the prepared receipts in the parent stream; child transfer
acknowledgements may follow. User-supplied credentials are borrowed and never
removed as an incidental consequence of removing a binding.

Revocation stops new use first. Cleanup then removes owned provider
registrations before destroying credentials required for that removal. A
subscription's dedicated generated signing material and owned secret destination
are separate cleanup obligations. Ambiguous provider results remain pending
until settled. A terminal removed fact requires verified absence of every owned
object and every adopted child, including writes completed by stale workers.

### Incoming acceptance

The pinned adapter verifies the original body bytes before parsing or
normalizing them. It owns provider-specific signature framing, verification
keys, freshness rules, challenge responses and allowed event types. There is
no unsigned fallback and no guessed universal signature scheme. Public-key
verification, where a connector supports it, uses explicitly registered trust
material; request-supplied keys cannot establish trust.

The endpoint determines the project. Caller-supplied project, account and
routing headers are untrusted. Only after verification may the adapter compare
provider account claims with the connection's admitted account. Verification
failure, unavailable credentials, expired policy or an unknown schema stops
acceptance. A challenge response is a transient protocol operation and cannot
create a subscription, activate a credential or dispatch an agent. A verified
event is still external data: a separate trigger policy decides whether it may
start a Session, and its payload cannot supply user approval or grant authority.

The acceptance identity combines project, endpoint and an adapter-established
event identity. An unsigned provider delivery header is not independently
trusted for replay prevention. The adapter must bind it to verified content or
derive a replay key from that content; its replay contract documents which.
An already accepted identity with the same semantic digest returns the earlier
acceptance. The same identity with different content is a conflict. Provider
redelivery must not change semantic identity merely because its attempt
timestamp or transport headers changed.

Verification precedes secret extraction. Provider bodies can contain bearer
capabilities, callback URLs or tokens. Before extracting material, the gateway reserves the receipt stream and
plans child credentials and sanitized artifacts. The adapter stores recognized
material through SecretStore and publishes only the sanitized, typed event with
opaque references. Acceptance adopts those resources; abandoned reservations
retain cleanup obligations and cannot be swept while acceptance is unresolved. If its schema cannot account for secret-bearing fields, admission
fails; raw provider bodies are not a generic durable escape hatch. Ordinary
business payloads remain confidential data with project access and retention
policy, even after authentication material is removed.

Success is acknowledged only after the sanitized payload is durably available
and its acceptance event is committed. Pending duplicate claims are not
success; recovery must find the committed receipt or resume publication. The
receipt contains the event identity, sanitized payload digest/reference and
verification metadata, never authentication headers or secret values. Payload
retention covers the processing and replay window. Publication failure returns
a retryable provider response according to the adapter, without pretending the
event was accepted. Downstream processing remains at least once.

Replay retention is explicit and at least the adapter's accepted redelivery
window. A timestamp-free provider cannot be given an invented freshness
guarantee. Such an adapter must state its bounded duplicate-suppression window
and the possibility of replay beyond it, or admission rejects a policy that
requires stronger guarantees.

### Outgoing event contract

Only versioned event types from the platform's exposed event catalog may be
selected. The dispatcher checks project ownership and disclosure policy before
creating a delivery. It does not forward arbitrary internal events. Each event
type pins a serializer with a secret-free external schema. A payload reference
names immutable bytes, their digest, size and expiry. Delivery signs and sends
those exact bytes; retries never reconstruct them from current resource state.

Platform event subscriptions use the Standard Webhooks HMAC-SHA256 `v1`
profile. Each subscription has its own randomly generated 32-byte shared key.
A future ephemeral generation exchange exposes that key once in its winning
direct response to the authorized recipient; subscription creation is metadata-only
and attaches its admitted binding. Admission verifies generation provenance and
exclusive subscription association instead of trusting a caller's claim that a
key is unique. Subsequent reads never return it. This chooses interoperability with
shared-secret receivers. It does not claim non-repudiation: the receiver can
also produce a valid MAC. Non-exportable asymmetric signing remains a distinct
KeyManagement capability, not a private key placed in a generic secret field.

The worker preserves the delivery ID and payload across retries, creates a new
attempt timestamp, and signs the exact Standard Webhooks frame. HTTP uses its
standard webhook headers. Signatures are computed for an attempt and not
persisted for replay. A previous key may be dual-signed only during an explicit
authorized overlap; revocation overrides that overlap. Provider-specific sink
formats, such as posting a chat message to a SaaS webhook, are connector
operations with their own pinned schemas, not this platform event protocol.

### Destination admission

A destination fixes an HTTPS origin and either a non-secret public path or a
binding to a secret callback URL. Public paths cannot carry query parameters,
fragments, user information or credentials. The resolved secret URL must have
the admitted origin. It is never included in metadata, request logs, audit or
error details. Endpoint-token rotation may preserve identity only if the
adapter verifies that the receiver security boundary is unchanged.

Before each attempt, the worker applies current DNS and network policy and
connects to the validated address with TLS verification. Redirects are errors.
Neither signing keys nor payloads follow them. Private destinations require a
deployment-attested policy under
[ADR#0066](./0066-secret-service-and-connection-boundaries.md); a subscription
cannot self-authorize access to internal networks.

### Delivery, retries and recovery

A delivery is unique for subscription, subscription configuration and source
event. Duplicate scheduling returns the existing delivery. A leased attempt
has its own ID, monotonic attempt number, owner and fencing token. Completion
must match the current lease. A stale worker cannot finalize a newer attempt.
Lease fencing protects platform state; it cannot cancel an HTTP request already
received by the remote system.

Delivery is at least once, unordered. A network failure after transmission has
an unknown remote outcome. Automatic and manual retries retain the delivery ID
so recipients can deduplicate; neither reports exactly-once side effects.
Manual replay repeats authorization and payload-availability checks and never
revives a revoked subscription. Sending to a replacement endpoint is a newly
authorized delivery, not a retry that changes destination under the old ID.

The platform default is a 72-hour delivery deadline, at most 30 attempts, a
30-second request timeout, and exponential backoff with full jitter starting
at five seconds and capped at six hours. Tenant policy may narrow these bounds.
The deadline and attempt limit both apply. Retry-After is honored only within
the remaining deadline. Responses in the 2xx range complete delivery; 410
disables the subscription; redirects and other failures use typed outcomes.
Rate limits, transient server errors and transport failures may retry. Permanent
authorization, destination or schema failures wait for explicit correction,
not a tight retry loop.

Exhaustion and payload expiry become retained terminal outcomes visible to
operators. They are not silent broker drops. An expired payload cannot be
reconstructed and called a replay. Resume after repair is an explicit command
with its own audit fact and a bounded new replay deadline. Attempt records
contain bounded status/failure metadata, not response bodies that might echo
credentials. Revocation prevents new sends once observed and remains subject
to the bounded propagation contract in
[ADR#0049](./0049-revocation-latency-target.md); it cannot recall an in-flight
remote request.

## Consequences

- Incoming and outgoing traffic share custody and authorization rules but have
  different resources, lifecycle events and recovery behavior.
- Provider verification is versioned adapter behavior; outgoing platform
  events have a named interoperable wire profile.
- A delivery can be retried safely at the platform state boundary, while remote
  side effects still require receiver idempotency.
- Existing gateway raw publication requires provider-by-provider migration for
  secret-bearing payloads. The schemas do not claim that migration is complete.
- All new contracts are staged protobufs excluded from Rust generation. Runtime
  enforcement, provider conformance and deployment proofs remain implementation
  acceptance work, not evidence supplied by schema compilation.

## References

- [Secret and connection contract reference](../reference/secret-and-connection-contracts.md)
- [Standard Webhooks specification](https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md)
- [GitHub webhook guidance](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks)
- [Slack request verification](https://docs.slack.dev/authentication/verifying-requests-from-slack/)
