# Workload Identity

This reference defines the proposed deployment profile and its acceptance gates
under draft [ADR#0098](../adr/0098-attested-workload-identity.md).
The checklists distinguish implemented consumer behavior from the issuer,
provider and deployment acceptance still required before enabling the profile.

For terminology and component responsibilities, read
[Workload identity and portable trust](../architecture/workload-identity.md).
The [federation proposal](../research/workload-identity/federation.md) separates
native SPIFFE domain trust, inbound external workload admission and outbound
provider exchange. The [adapter roadmap](../research/workload-identity/adapter-roadmap.md)
records portability, custody and implementation dependencies. Both remain
research proposals, including any amendment to current production seal policy.
The [configuration contract reference](./workload-identity-configuration.md)
maps the protobuf records and the identity-aware workflow version gate.

## Runtime foundation and limits

| Boundary | Foundation | Enablement gate |
| --- | --- | --- |
| Egress workload identity | Explicit Unix Workload API consumer, governed application mapping, acknowledged activation and live credential/bundle replacement | Attestor, issuer, isolation and recovery acceptance in the selected deployment |
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
not track absent trust anchors. The explicit Workload API profile additionally
withdraws sessions when its captured identity, registration or bundle changes.

## Workload API consumer

The `trogon-workload-identity` crate consumes the exact configured local identity
over a Unix socket. It validates the SVID, matching private key, certificate
path, one-hour certificate ceiling and exact approved root fingerprints.
Registration maps the authenticated peer SPIFFE ID to its tenant, project,
role and canonical application ID. The URI path cannot choose an application
principal or grant a destination permit.

A bounded candidate becomes usable only after its platform activation record
is durably acknowledged and the current source, registration and trust
generations still match. Replacing or withdrawing authority invalidates captured
material leases and cached TLS configuration. A lease ends at the earlier
certificate-path expiry or governed registration deadline, with a registration
lifetime ceiling of 330 seconds. A source disconnect requires a newer governed
registration before material can become usable again.

Complete snapshots that omit the local binding, remove every binding or remove
every approved root are valid withdrawals. They retain their registration and
trust revision floors. A later document cannot restore an older registration
or retired trust set by carrying a newer timestamp or registration revision.

The egress workload profile uses this consumer for each handshake and retains
its lease through request handling and forwarding. Both registration and policy
require dedicated, bounded, authenticated NATS streams. There is no file or
alternate-identity fallback. Follow the [deployment procedure](../how-to/deploy-workload-identity.md)
for configuration and stream isolation.

All workload-profile audit, policy and registration connections use the reply
prefix `_INBOX.egress.<registration_bucket>`, derived from the validated
registration bucket. Permit subscriptions only beneath that exact prefix.
Request API allowlists alone do not protect replies: every untrusted role in
the same account must also lack broad `_INBOX.>` or `>` subscriptions. Isolate
accounts when that account-wide constraint cannot be enforced. Trusted broker
administrators remain part of the security boundary.

Local integration tests use real Unix Workload API streams, verified TLS NATS,
JetStream activation and decision acknowledgements, and TLS-authenticated
CONNECT forwarding. They cover registration removal, policy withdrawal, source
`PermissionDenied` and an unavailable audit stream. These receipts establish
the consumer and enforcement paths, while the attestor, issuer, HSM, cluster
isolation, HA and restored-state checks below still require deployment evidence.

This consumer rejects every response containing CRLs because its supported
profile does not implement CRL signature and revocation processing. It does
not silently discard supplied revocation data. An issuer that requires CRLs
cannot enable this profile until that support is implemented and verified.
Governed registration/root withdrawal and bounded credential expiry remain
separate enforced controls.

## Original registration artifacts

`RegistrationArchive` preserves bounded original approval wrappers and public
bootstrap manifest bytes in separate digest-addressed native streams. Deployment
configuration supplies finite record and byte capacities. The immutable profile
uses replicated file storage, rejects new records at capacity and retains existing
records without expiry or eviction. Its connection requires authenticated TLS and
preprovisioned archive streams with separate archive permissions.

Use `RegistrationArchive::connect` with Writer credentials and
`RegistrationArchive::connect_reader` with Reader credentials. Reader connections
use their own reply namespace and reject `create_once` before any native operation.

Artifact input becomes immutable original bytes and their SHA-256 digest. Its
kind selects bounds and storage scope; it does not validate protobuf structure,
an approval signature, a contract definition or manifest lineage.
These artifacts are public bootstrap/control metadata. Private seeds and
credentials remain under canonical sealing.

`create_once` returns either a receipt for the exact submitted bytes and actual
native ACK revision, or a separate observation of an existing exact record. The
latter does not manufacture another ACK. A later read cannot replace a missing
ACK. Uncertain writes return no create receipt and are not retried automatically;
an ACK followed by failed source checks is reported as acknowledged but unusable.

`observe_exact` reads the digest-addressed original bytes. Its observation's
`revalidate_original` checks the same retained connection, stream creation,
immutable profile and exact original revision. These are bounded observations,
not continuous authority. Configured scopes and recovery fences do not verify an
approved account/JWT mapping or independently governed restore fence. Storage
evidence cannot construct a live approval, grant or execution capability.

## Governed registration publication

The `trogon-workload-registration` service accepts independently signed,
canonical registration snapshots for its configured scope and recovery fence.
Signatures bind the operation purpose, exact trust domain and exact canonical
protobuf bytes, including tenant, project and recovery fence.
Every renewal needs a fresh approval and a strictly increasing registration
revision. Changing roots also advances the trust revision. Changing bindings
or roots requires an acknowledged whole-scope denial before a new grant.
There is no automatic renewal from an earlier approved snapshot.

Each scope and recovery fence has separate control and live streams. Both
retain only the latest complete snapshot on one exact subject, use replicated
file storage and refuse deletion or purge. The live stream expires its data;
the control stream retains the ownership and revision head. Only an empty,
never-claimed scope can be claimed. Existing sequence history refuses a new
writer even after its live data expires.

Grant publication requires a durable platform audit acknowledgement and a
broker comparison against the captured live sequence. Emergency withdrawal
publishes the signed complete denial to the live stream first. Later audit or
bookkeeping failure does not remove that denial. A conflicting publication,
uncertain effect or cancelled update permanently poisons the writer handle.
The captured authenticated broker connection must also remain unchanged across
reads, audit waits and publication acknowledgements. Reconnection does not
transfer ownership to the replacement connection.
The native SDK can retain an already-enqueued command after its caller times
out. Protected connections therefore quarantine later dials for 335 seconds,
longer than the maximum 330-second approval lifetime. Any pre-disconnect
snapshot expires before it can reach a replacement transport. Connection checks
still poison the writer; a timeout does not retract an uncertain effect.
Recovery requires independently advancing the fence, provisioning new streams,
revoking old publication credentials and updating consumers.

Provisioning, publication, emergency withdrawal and workload readers use
separate credentials and reply namespaces. Every credential that can write
the authority streams is a trusted control-plane credential. NATS subject
permissions constrain its scope, but cannot validate snapshot bodies or make
that credential cryptographically denial-only. The trusted service verifies
approvals and enforces the withdrawal command. Protect its code, controller,
approval-key custody and broker credentials independently.

See the [administrative runner reference](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/platform/trogon-workload-registration/README.md)
for required configuration, bounded signed framing, command roles and terminal
failure categories. Local broker tests establish these code paths; independent
recovery governance and deployed HA/restore acceptance remain required.
These time bounds also require the deployment's clock-integrity controls;
restored state and clocks cannot substitute for an independently advanced fence.

The [registration operations procedure](../how-to/operate-workload-registration.md)
connects these controls into provisioning, renewal, withdrawal and recovery.
The [trusted-host integration procedure](../how-to/integrate-workload-identity.md)
explains how a consumer retains material and peer deadlines through actual I/O.

## Egress readiness

`Server::readiness()` checks the running workload profile, current approved
material and governed registration, a current complete policy, audit admission
capacity and a fresh durable audit acknowledgement. It checks authority again
after that acknowledgement. Compatibility profiles and a stopped or aborted
runtime cannot report workload readiness. A valid deny-all policy remains a
usable policy; readiness does not grant a destination permit.

The binary exposes `GET /readyz` on `127.0.0.1:8081` by default. Configure
`--readiness-listen` or `TROGON_EGRESS_READINESS_LISTEN` for a restricted deployment
probe address. Responses contain only status and a fixed body, with
`Cache-Control: no-store`. The endpoint bounds concurrent workers, request-head
bytes and total connection time. Its probes retain audit capacity through actual
write completion, including when the HTTP caller disconnects or times out.

This health signal establishes current consumer and enforcement usability.
Artifact provenance, attestor correctness, signer custody, retained historical
audit, cluster isolation, failover and restore acceptance still require the
deployment evidence below.

## Enforcement mode and connection events

The egress proxy enforces by default. `--enforcement-mode` or
`TROGON_EGRESS_ENFORCEMENT_MODE` selects `enforce` or `dry_run`, and any other
value refuses startup. Dry run changes one answer: an authenticated caller whose
identity holds no permit for the requested destination is forwarded instead of
refused. The proxy then emits a warning event named `egress_would_deny` and the
audit record carries the dry run enforcement mode with outcome `would_deny`.
Authentication, the X.509-SVID checks, the private address guards and refusals
because policy is unavailable are the same in both modes. Dry run is for
observing a rollout, never a standing configuration.

Every accepted mTLS connection logs one `peer_connection_accepted` event with the
full peer SPIFFE ID, the peer address and the credential source, `workload_api` or
`static_file`. A refused certificate logs the SPIFFE ID it claimed, if it claimed
one, with the refusal reason. These events are the service dependency record: they
show which identity actually calls the proxy, to compare with the expected one.
The same acceptance increments the OpenTelemetry counter
`egress_proxy.connections.accepted` with the attribute
`egress_proxy.credential_source`.

The static-file listener checks its client trust anchors when it loads them. An
anchor that carries a URI subject alternative name must name exactly the
configured trust domain, and an anchor naming another trust domain or carrying
more than one URI subject alternative name refuses startup. An anchor with no URI
subject alternative name, such as an upstream root, is accepted, so the file
must still hold only this trust domain's anchors.

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

## SPIFFE ID convention

The [SPIFFE ID specification](https://github.com/spiffe/spiffe/blob/main/standards/SPIFFE-ID.md)
is the normative source for what an ID may contain. This convention narrows it for
workloads that Trogon registers. Settle it before the first registration entry, because
renaming an ID later means a new entry, a new authorization policy and a rotation of every
peer that pins the old one.

- **Trust domain per environment.** Each environment has its own trust domain and roots.
  The environment is part of the trust domain, such as `production.identity.example.org`,
  and never appears in the path.
- **Local workload path.** `/<owning-group>/<service>`, optionally followed by further
  stable qualifiers, for a total of two to four segments. A segment matches
  `[a-z0-9]([a-z0-9_-]{0,61}[a-z0-9])?`, so it is lowercase only. An example is
  `spiffe://production.identity.example.org/platform/egress`.
- **Case.** Nothing is case-folded. The scheme and trust domain must already be lowercase,
  and the path is case-sensitive. The runtime types and the trust manifest check refuse a
  non-canonical ID instead of normalizing it, and policy compares the exact canonical
  string. The lowercase-only segment rule removes any case ambiguity for local IDs.
- **Stable values only.** A path never contains a pod name, node name, UID, image digest,
  address or timestamp. Those change with every deployment and belong in selectors.
- **No authority in the path.** The path never selects a tenant, project, role or `AgentId`;
  [registration](#attestation-and-registration) supplies those from governed state.
- **Enforcement.** The deployment compiler refuses an ID that breaks this convention. See
  [Prepare a workload identity deployment](../how-to/deploy-workload-identity.md).
- **Wire contract.** Every SPIFFE ID field that names a workload requires a lowercase trust
  domain of at most 255 bytes and a non-empty path with no empty, `.` or `..` segment, no
  trailing slash, query or fragment, within 2048 bytes. The wire rule also accepts the
  upper-case path characters that the specification allows, because foreign IDs are not
  ours to constrain.
- **Static egress alias.** The egress proxy's `/agent/<agent-id>` alias is a
  separate legacy profile that does not follow this convention. It is the static
  compatibility slice, not the workload identity profile. It still is a SPIFFE ID: the
  proxy refuses to render or accept an alias whose agent segment holds anything outside
  letters, digits, `.`, `-` and `_`, so a `did:key:` agent id has no static alias and
  appears in policy documents and registrations only.
- **Foreign domains.** A foreign trust domain keeps its own conventions, and Trogon matches
  its IDs exactly rather than normalizing them.

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
- [x] Gate candidate SVID/key/bundle activation on validated registration, an
  acknowledged durable activation fact and a final live-generation check, with
  a bounded candidate queue and retained in-process revision floors.
- [ ] Independently advance the protected recovery fence across restore and
  collect native SPIRE issuance events. Ordinary issuer logs do not implement
  the platform activation gate or prove pre-signing acknowledgement.
- [x] Harden the SPIRE server configuration:
  - Keep `admin_ids` short, owned and reviewed; every entry is a standing
    administrator.
  - Leave `allow_nonconforming_agent_ids` false.
  - Set `plugin_checksum` for every external plugin.
  - Disable the Delegated Identity API, or limit its delegates to named trusted node
    components and handle recycled process IDs.
  - For a regulated or high-assurance profile, chain an `UpstreamAuthority` to a
    hardware-protected enterprise CA. The compiler renders the `aws_pca` plugin from
    `upstream_authority` and requires it when `assurance_level` is `regulated`; a
    `disk` upstream authority is refused. A `standard` profile may stay self-rooted
    with KMS-held keys.

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

- [x] Consume Workload API SVID and bundle updates; rotate before expiry within
  the one-hour certificate lifetime ceiling. Replace key/certificate/bundle
  state atomically and retire old trust according to a bounded overlap policy.
- [x] Use standard X.509 path validation plus the SVID leaf, URI and trust-domain
  checks. Separate legacy mTLS pins from SPIFFE registration authority.
- [x] Require a full certificate-authenticated handshake for each new egress
  connection. Disable stateful TLS session storage and TLS 1.3 session tickets
  so resumption cannot skip current client-certificate verification.
- [x] Treat Workload API updates as complete snapshots. Remove omitted
  identities/bundles and handle `PermissionDenied` without waiting for audit;
  never merge withdrawn authority into the next snapshot or switch identities
  because a default entry changed.

Verification:

- [ ] Reject non-CA intermediates, invalid CA key usage, exceeded path lengths,
  unknown critical extensions, invalid leaf purpose, expired chains, multiple
  URI identities and unexpected trust domains.
- [ ] Verify rotation with old and new connections, missed update/reconnect,
  wrong-domain bundles, compromised-root removal and failed renewal, including
  rotation under load at the connection limit.
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
- [x] Bound detached egress audit workers and retain each slot until its actual
  write finishes, even when the requesting connection is cancelled. Refuse
  excess writes immediately and close admission before draining on shutdown.
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

## JWT-SVID verification

X.509-SVID with mutual TLS stays the default workload credential. A JWT-SVID is for a
hop where a layer-7 intermediary terminates TLS. It is a bearer token, so it is never
forwarded to another service; each hop gets a token for its own audience.

`trogon-workload-identity` provides the only accepted path for a JWT-SVID:
`JwtSvidVerifier::verify(token, hop)` returns a `VerifiedJwtSvid` (workload ID,
audience, expiry, hop kind) or a typed `JwtSvidError`. The verifier is built with one
expected `JwtAudience` and a `JwtBundleSource`.

JWT bundles are delivered and rotated through the Workload API, like X.509 bundles. A
`JwtBundleDriver` reads the `FetchJWTBundles` stream with the same reconnect, backoff,
bounds and redaction rules as the X.509 stream, and the verifier reads the current set on
every call, never a snapshot. Each update replaces the whole set, so an authority absent
from an update stops being trusted at once. Bundles are one `JwtBundle` per trust domain
and are never merged. An update with any invalid entry (no keys, a bad key, an unparsable
trust domain, a duplicate, an excess of bundles or bytes) is refused whole, the last good
set is kept, and the stream is reconnected.

The verifier fails closed, before it reads the token, when the source cannot vouch for
its set:

- `bundles_not_delivered`: no set has arrived yet.
- `bundles_stale`: the stream has been lost for longer than 30 seconds
  (`JWT_BUNDLE_MAX_STALENESS`). A live stream is never stale. The bound equals the default
  token leeway and covers a reconnect or an agent restart, so a removed authority can
  outlive a broken stream by at most that long. The X.509 side withdraws material as soon
  as its stream ends; the JWT side keeps a short grace because each verification is
  independent and a brief reconnect should not refuse every request.

A fixed set exists only for tests (`JwtBundleSource::static_for_tests`, behind the
`test-support` feature).

The opt-in test `devops/kubernetes/spire/test_jwt_bundle_live.py` proves the bundle
source and the verifier against a real SPIRE 1.16.0 agent: the bundle arrives through
`FetchJWTBundles`, a JWT-SVID minted by the agent verifies for its audience and is
refused for another, and the source goes stale 30 seconds after the agent stops.

The hop rule is enforced by the type, not by guidance. `verify` takes a
`ConfidentialHop`, and the only constructors are `ConfidentialHop::mutual_tls`, from the
`PeerEvidence` of an authenticated mutual-TLS peer, and `ConfidentialHop::unix_socket`,
from a connected local Unix domain socket (the one accepted exception; peer credentials
come from the kernel). A token cannot be verified without one of them. A mutual-TLS hop
is refused as `hop_expired` once its peer evidence expires. `VerifiedJwtSvid::hop()`
returns the `HopKind` (`mutual_tls` or `unix_socket`) for logging.

`JwtAudience` is an absolute URI naming one specific service, such as
`spiffe://prod.example.com/reports`. It cannot be built from an empty value, a value with
whitespace or control characters, a bare trust domain (`example.org`, `spiffe://example.org`,
`spiffe://example.org/`), a bare word such as `production`, or a URI without an
authority. Rules, applied in this order after the hop check:

- JWS compact serialization only: three base64url segments, bounded size.
- `alg` is RS256/384/512, ES256/384/512 or PS256/384/512, checked before any key lookup.
  `none`, HMAC and EdDSA are refused. All nine algorithms verify, and an EC key verifies
  only the algorithm of its curve: ES256 with P-256, ES384 with P-384, ES512 with P-521.
- Header parameters are `alg`, `typ` and `kid` only; `typ`, if present, is `JWT` or `JOSE`.
- `sub` is a canonical workload SPIFFE ID with a path. Its trust domain selects the
  bundle; an unknown trust domain is refused and no other bundle's key is tried.
- The signature verifies under the key named by `kid`, or, without `kid`, under a key of
  that bundle whose type fits `alg`. Bundle keys come from JWKS entries with
  `use: jwt-svid` and a `kid`; RSA keys are at least 2048 bits, EC keys P-256, P-384 or P-521.
- `aud` is present and names exactly one audience, the expected one. A token scoped to
  several audiences is refused even when one of them matches.
- `exp` is present, not past (30 seconds of leeway by default, at most 60) and at most
  300 seconds ahead by default (configurable up to 3600). `nbf`, if present, has passed.
  The ceiling is local policy; the specification sets no upper limit.

Each refusal logs a `warn` event with a stable `reason` code and never the token or key
material.

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

### Lifetime ceilings

These ceilings bound different things and are not interchangeable:

| Ceiling | Bounds | Source |
| --- | --- | --- |
| 330 seconds | Governed registration lease, egress session and policy backstop | [ADR#0049](../adr/0049-revocation-latency-target.md) |
| 3600 seconds | X.509-SVID lifetime | [ADR#0098](../adr/0098-attested-workload-identity.md) |
| 300 seconds | JWT-SVID assertion lifetime (`max_assertion_lifetime`) | [Wire bounds](./workload-identity-configuration.md#proposed-wire-bounds) |

The egress proxy's legacy static-file mode applies the same 3600-second ceiling. It
refuses a client certificate whose validity (not-after minus not-before) exceeds it during
the TLS handshake and logs `reason = client_certificate_lifetime_exceeded`. It also checks
its own certificate when the configuration loads, and startup fails with a typed error
rather than serving it. The ceiling is a constant, not a setting. A legacy long-lived
static certificate, on either side, must be reissued with a validity of one hour or less
before this proxy accepts it.

Static-file mode also holds both certificates to the X.509-SVID key profile: not a CA,
`digitalSignature` set, neither `keyCertSign` nor `cRLSign`, and, when an extended key
usage is present, both `serverAuth` and `clientAuth`. A client certificate that breaks it
fails the TLS handshake. The proxy's own certificate is checked when the configuration
loads, so a certificate issued with only `serverAuth` stops startup with a typed error.
Reissue such a certificate with both usages, or with no extended key usage, before
upgrading.

### Measurements

Track these to show the rollout is making progress. They are measures, not gates; the
gates are the checklists above. The
[deployment procedure](../how-to/deploy-workload-identity.md#measure-progress) names
the source of each number.

- Share of service-to-service calls authenticated by SVID.
- Static service credentials retired, counted against the inventory taken at the start.
- Registration entries with a single broad selector. The target is zero.
- Median SVID lifetime, and rotation failures per week.
- Time to revoke, from entry deletion to the expiry of the last valid SVID. The
  X.509-SVID ceiling of 3600 seconds bounds it, and platform use ends sooner, within the
  330-second registration bound.

The [connector conformance gate](./connector-conformance.md) remains necessary
for provider integrations. No certificate fixture, local library test or draft
ADR substitutes for these deployment and provider checks.
