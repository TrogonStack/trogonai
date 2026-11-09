# Workload Identity

This reference defines the proposed deployment profile and its acceptance gates
under draft [ADR#0098](../adr/0098-attested-workload-identity.md).
The checklists identify work that must be implemented and verified; they are not
a claim that a production SPIRE integration exists in this repository.

## Runtime foundation and limits

| Boundary | Foundation | Enablement gate |
| --- | --- | --- |
| Egress workload identity | TLS certificate validation, provisional agent URI mapping and finite certificate-bound sessions | Workload API integration, authorized identity mapping and credential/bundle rotation |
| Egress authorization | Exact-destination admission, optional authenticated live KV authority, generation-bound audit and finite sessions | Governed producer, independent recovery fencing and end-to-end revocation acceptance |
| NATS admission | Operator accounts, verified credentials and required explicit caller-to-tenant membership authority | Governed membership operations and explicit SPIFFE profile if selected; legacy DN-based mTLS is not that profile |
| AAuth issuance | Person and Access Server libraries with bounded validity, deferred-request revalidation and injected policies | Real consent/policy adapters and durable bounded operation |
| External federation | Draft ADR0053 exact-match issuer/subject/audience design | Attested assertion issuer, provider registration and real exchange acceptance |
| Provider access | Draft connector custody, grant, relay and recovery contracts | Implemented custody/relay plus provider and isolation acceptance |

The callout binary requires a bounded operator-managed membership registry and
reads it again before every mint. Removing an entry prevents new admission;
missing or invalid authority cannot reuse a cached permit. This is an explicit
trusted configuration adapter, not a deployed HA membership control plane.
Existing broker credentials remain subject to their own expiry and the separate
revocation contract.

The egress listener also bounds active TCP, TLS-handshake and tunnel workers.
The default is 128, configurable from 1 through 4096 with `--max-connections`
or `TROGON_EGRESS_MAX_CONNECTIONS`. Excess TCP connections close before a worker
is created, without a waiter queue. A connection retains its slot through its
handshake and authenticated lifetime; completed workers release capacity.
These limits are resource policy, not measured throughput or availability.

The callout binary shares a finite admission budget across its direct and bridge
routes. The default permits 16 workers, acquired before dispatch and retained
through reply publication. Excess requests receive an opaque refusal without a
credential or a queued worker. This local bound does not establish replica
capacity, broker ingress limits or durable service availability.

Egress sessions end at the earlier presented-certificate expiry or a 330-second
monotonic ceiling. Destination and selected-address policy are checked after
waits and while forwarding. Live policy additionally supplies a finite producer
deadline and update notifications. Explicit static-file compatibility still
retains its startup policy across reconnects. The presented-chain deadline does
not track absent trust anchors or implement Workload API bundle withdrawal.

## Live egress authority

Select `--policy-bucket` and a nonzero `--policy-incarnation` instead of
`--policy-file`. The corresponding environment variables are
`TROGON_EGRESS_POLICY_BUCKET` and `TROGON_EGRESS_POLICY_INCARNATION`.
Production startup requires verified NATS TLS and client authentication under
the standard NATS configuration. It refuses plaintext, anonymous access and
JWT-only authentication without a holder seed or a client certificate. The
source's tenant and project must match the configured runtime and audit scope.
It never falls back to a static file.

Provision the bucket independently. It must have history 1, maximum value size
at most 256 KiB, aggregate storage at most 1 MiB and positive maximum age no
greater than 325 seconds, with no mirror or source stream. Bucket names are
limited to 64 ASCII letters, digits, underscores or hyphens. Use a separate
bucket per tenant/project and restrict the reader to its stream-information and
message-read APIs; leader reads grant bucket-level visibility. Keep publishing,
stream administration and recovery permissions outside the workload reader.

The exact key is `t{hex_utf8(tenant)}.p{hex_utf8(project)}` using lowercase hex,
also available through `KvPolicyConfig::key()`. Each value is a complete JSON
document with these fields:

| Field | Contract |
| --- | --- |
| `tenant`, `project` | Exact configured scope |
| `incarnation` | Nonzero integer matching independently provisioned configuration |
| `generation` | Nonzero integer that strictly increases for each new policy or renewal |
| `issued_at`, `valid_until` | Integer Unix seconds; `issued_at <= now < valid_until`, with positive lifetime at most 325 seconds |
| `policy` | Exact-destination TOML policy; an empty live policy withdraws every permit |

Unknown or duplicate fields and malformed policy are refused. Documents are
limited to 256 KiB, 1024 agents and 4096 unique permits. Malformed documents
produce closed error categories without retaining raw parser input in errors.

The reader polls leader-backed APIs every second with a one-second refresh
deadline. Missing, deleted, invalid, disconnected or unavailable authority
withdraws the active policy. Complete updates replace earlier permits; they do
not merge them. Duplicate delivery cannot extend the original monotonic lease
or restore a withdrawn generation. Recovery requires a newer producer
generation, and a recreated stream is not adopted automatically.

Policy observations bind the permit, producer generation and deadline together.
Audit records name the bucket, incarnation and applied generation; a policy
change while an allow record awaits acknowledgement prevents that old
acknowledgement from opening a tunnel. Update notifications wake established
tunnels, including when a replacement shortens their deadline. The producer
deadline also bounds DNS, connection, audit and forwarding waits.

The reader does not implement the authoritative publisher or a durable external
recovery fence. Recovery governance must advance the independently provisioned
incarnation; local revision counters cannot identify an entire restored system.
A publisher must not renew stale grants as fresh policy. Tests of this reader
and its authenticated runtime path do not establish HA, production attestation
or ADR0049's end-to-end p99 target.

## Attestation and registration

Implementation:

- [ ] Integrate a pinned SPIRE release and supported node/workload attestors for
  each admitted runtime. Configure exact registration selectors and parent
  constraints; resolve transport identity to tenant, role and application identity
  from governed state.
- [ ] Keep registration administration separate from workload callers. Require
  audit and independent approval for trust/selector widening and signing-authority
  changes. Emergency denial is immediate and reviewed afterward.
- [ ] Bind bootstrap and renewal to verified runtime identity and key possession,
  with freshness and replay protections appropriate to the chosen attestor.
  Do not use shared join tokens or signing-key possession as attestation.

Verification:

- [ ] A copied application key, caller-chosen SPIFFE ID, another tenant's runtime,
  broad selector, forged evidence or wrong key cannot obtain the identity.
- [ ] Expired/replayed evidence and unavailable attestors refuse issuance or
  renewal without a static-credential fallback.
- [ ] Registration revocation, redeployment and restore cannot reactivate a
  previous runtime or transfer a Connection without new authorization.

## Issuer bootstrap, custody and isolation

Implementation:

- [ ] Configure production signing through a supported HSM-backed key manager
  and deployment IAM. Keep platform bootstrap, SPIRE administration and tenant
  credential custody distinct; no direct workload OpenBao access is implied.
- [ ] Deploy redundant servers with a durable datastore, authenticated restricted
  administrative endpoints, backups and a fenced recovery procedure.
- [ ] Restrict the Workload API and its key material to the trusted supervisor or
  adapter. Enforce tenant and process/VM isolation independently of SPIFFE.
- [ ] Gate candidate SVID/key/bundle activation on validated registration, an
  acknowledged durable activation fact and a final live-generation check. Fence
  generations across restore and bound the candidate queue. Collect native
  SPIRE issuance events separately; ordinary issuer logs do not implement this
  platform gate or prove pre-signing acknowledgement.

Verification:

- [ ] Another tenant, guest process or untrusted agent cannot call the privileged
  identity endpoint, read keys or administer registrations.
- [ ] Signer/IAM loss, datastore loss, failed audit and restored stale state do
  not activate unauthorized identities or silently select another signer.
- [ ] Audit outage, revocation during an audit append, duplicate/reordered update
  and restored acknowledgement cannot activate a stale candidate or extend the
  old credential, even while the Workload API stream remains healthy.
- [ ] Demonstrate issuer failover and key-compromise recovery without widening
  trust; validate any hardware or compliance claim against the actual deployment.

## Certificate and trust lifecycle

Implementation:

- [ ] Consume Workload API SVID and bundle updates; rotate before expiry within
  the one-hour certificate lifetime ceiling. Replace key/certificate/bundle
  state atomically and retire old trust according to a bounded overlap policy.
- [ ] Use standard X.509 path validation plus the SVID leaf, URI and trust-domain
  checks. Separate legacy mTLS pins from SPIFFE registration authority.
- [x] Require a full certificate-authenticated handshake for each new egress
  connection. Disable stateful TLS session storage and TLS 1.3 session tickets
  so resumption cannot skip current client-certificate verification.
- [ ] Treat Workload API updates as complete snapshots. Remove omitted
  identities/bundles and handle `PermissionDenied` without waiting for audit;
  never merge withdrawn authority into the next snapshot or switch identities
  because a default entry changed.

Verification:

- [ ] Reject non-CA intermediates, invalid CA key usage, exceeded path lengths,
  unknown critical extensions, invalid leaf purpose, expired chains, multiple
  URI identities and unexpected trust domains.
- [ ] Verify rotation with old and new connections, missed update/reconnect,
  wrong-domain bundles, compromised-root removal and failed renewal.
- [x] Exercise real TLS 1.2 and TLS 1.3 handshakes with shared client session
  state. Subsequent egress connections require full handshakes; an explicit
  resumption control demonstrates that the fixture can resume both protocols.
- [ ] Verify identity/bundle redaction and `PermissionDenied`, including while
  audit is unavailable. Explicit removal and compromise override rotation
  overlap, and a reconnect cannot restore a removed generation.
- [ ] No credential, cached bundle or established connection survives beyond its
  applicable expiry or configured authority-staleness bound.

## Authorization, delegation and deferred issuance

Implementation:

- [ ] Independently check operation, tenant, destination, caller role, binding,
  audience and live grant after authentication. Reevaluate policy on a deferred
  request and recheck time-dependent input authority immediately before minting.
- [x] Require authoritative caller-to-tenant account admission before minting a
  broker credential. Signing-key availability and a tenant-scoped identity hash
  do not establish membership; absent admission authority refuses the mint.
- [x] Bound concurrent broker admission before credential verification and
  registry reads, share the budget across direct and bridge routes, and refuse
  excess work without an unbounded waiter queue or stale admission fallback.
- [x] Bind live egress authority to the exact runtime tenant/project and
  independently configured incarnation. Require authenticated TLS for the
  production reader and retain the evaluated generation through permit audit.
- [x] Bound egress connection workers before spawning, including unauthenticated
  handshakes, and retain capacity until the connection ends.
- [ ] Keep workload federation separate from delegated user consent; intersect
  relevant policy and delegation before granting on-behalf-of authority.
- [x] Bound AAuth output validity by the agent and any bound subagent/upstream
  delegation, independently of a resource challenge's exchange deadline.
- [x] Bound derived broker user JWTs by verified OIDC/certificate-path expiry
  as well as configured admission TTL, and recheck expiry immediately before
  minting. Preserve finite evidence separately from serialized identity claims.
- [x] Refuse zero or subsecond broker lifetimes at the shared mint boundary,
  including legacy API-key and direct mint calls. Encoding an integer-second
  token cannot round a configured maximum up into new authority.
- [ ] Bound pending state, retention, replay protection and concurrency in the
  real service; an in-memory library store is not HA acceptance.

Verification:

- [ ] A valid certificate without the requested permit still fails. Tenant,
  role, binding, audience, grant generation and holder-key substitutions fail.
- [x] A valid OIDC or mTLS caller without membership cannot obtain a configured
  tenant account's broker credential by naming that account. Test identity
  namespacing and account admission as distinct boundaries.
- [x] Hold admission workers over real broker requests, verify both routes
  promptly refuse excess work without dispatch or a JWT, then release the held
  work and verify fresh admission recovers under the same shared budget.
- [x] Exercise production egress startup through real NATS TLS, required client
  certificates and JetStream. Synchronize initial connection before reading
  authority; complete removal changes subsequent authenticated admission.
- [x] Change live policy while permit audit awaits acknowledgement. An old
  acknowledgement cannot open a new tunnel under a different generation.
- [x] Hold real TCP/TLS connections at the configured worker limit, refuse
  excess connections promptly and verify capacity recovers after release.
- [x] Advance the clock while a request awaits claims, interaction or approval;
  expired inputs cannot produce a fresh grant. A slow policy cannot mint past
  the authority it evaluated.
- [x] Auth token expiry never exceeds the applicable agent/delegation validity;
  a short resource challenge alone does not unnecessarily shorten the grant.
- [x] A source expiring during credential verification or dispatcher work cannot
  mint a fresh broker JWT. A shorter selected intermediate/root expiry also
  bounds admission, while a shorter configured TTL remains effective.
- [x] Reject future not-before dates under the selected clock policy, malformed
  present date claims and unsupported critical JOSE extensions. Cover dates
  within a decoder's default leeway, exact activation/expiry and numeric limits;
  optional absent claims remain compatible with the reviewed protocol profile.
- [x] Require HTTPS issuer configuration, exact issuer matching, successful
  discovery/JWKS status and bounded responses, including chunked bodies. Reject
  cross-origin or credential-bearing JWKS URLs and redirect responses.
- [ ] Missing, revoked or narrowed user consent cannot be recovered from an
  earlier workload-authentication success or cached policy decision.

## Provider exchange and token protection

Implementation:

- [ ] Register the actual provider's exact issuer/subject/audience and allowed
  role/resource mapping. Prefer supported federation or asymmetric client
  authentication; preserve provider-required credential custody.
- [ ] Implement only reviewed sender constraints that both parties support.
  Keep RFC8705 certificate binding, DPoP and AAuth proofs distinct. A bearer-only
  provider needs a reviewed compatibility profile with its residual risk stated.

Verification:

- [ ] Exercise the real provider or an exact protocol test double for wrong
  issuer, audience, binding, scope and provider-account substitutions.
- [ ] Replay a sender-constrained token using another holder key: it fails at
  the relevant recipient. Successful client mTLS alone cannot pass this gate.
- [ ] Unsupported proof methods or lost binding keys cannot downgrade to bearer.
  JWT-SVIDs and bearer-only providers are never labeled sender-constrained.

## Cache, revocation and established sessions

Implementation:

- [ ] Never cache exchange assertions. Separate resource-token caches from JWKS
  and trust bundles; include tenant, credential source/account, generations,
  scope, audience and holder identity in resource-token cache isolation.
- [ ] Recheck live authority on every use. End local usability at the earliest
  applicable credential, grant, key and revocation bound.
- [ ] Apply live invalidations and bounded reevaluation to existing tunnels and
  sessions. Prove both ADR0049's normal-operation target and a hard staleness
  bound no greater than its 330-second backstop.
- [x] Apply complete live egress snapshots, notify existing tunnels of changes
  and enforce the exact monotonic producer deadline across waits and forwarding.
  Keep static-file compatibility separate from this lifecycle.

Verification:

- [ ] Identical AgentId/audience/scope on different tenants, Connections,
  accounts, apps, issuers or generations never shares an eligible credential.
- [ ] Lost invalidations, stopped projections, clock boundaries, key retirement,
  disconnection and restore cannot resurrect authorization or extend a tunnel.
- [x] Remove an egress permit over real KV: an established tunnel closes and a
  new connection is refused. A shortened replacement lease wakes an existing
  tunnel; duplicates cannot extend expiry or recover withdrawn authority.
- [ ] Offboarding distinguishes immediate local denial from already-issued
  external tokens, whose provider-controlled expiry/revocation remains visible.

## Deployment acceptance

Record pinned software/artifact identities, attestor configuration, trust domains,
signer/IAM evidence, registration and network policy, completed failure tests,
issuer failover, retention and recovery receipts. Exclude raw keys, attestation
assertions and tokens. A missing required capability blocks this profile;
compatibility mode cannot satisfy the gate by changing its label.
Verify artifact signatures and provenance against configured release trust;
a digest obtained without authenticated provenance is not sufficient evidence.

The [connector conformance gate](./connector-conformance.md) remains necessary
for provider integrations. No certificate fixture, local library test or draft
ADR substitutes for these deployment and provider checks.
