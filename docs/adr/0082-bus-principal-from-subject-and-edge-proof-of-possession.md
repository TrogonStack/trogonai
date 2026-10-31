---
number: "0082"
slug: bus-principal-from-subject-and-edge-proof-of-possession
status: accepted
date: 2026-10-30
---

# ADR#0082: Inside the Bus the Principal Is Derived from the Subject, and Proof of Possession Is Enforced Only at the External Edge

## Context

[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md)
decides who may connect to NATS and what each connection may publish and
subscribe to. It leaves open how a service that receives a message learns on
whose behalf the message was sent. On the A2A path every service answers that
from message headers today, and a header says whatever its publisher wrote.

The gateway resolves its caller in `resolve_gateway_caller_identity`
(`rsworkspace/crates/a2a/a2a-gateway/src/jwt_caller_identity.rs:220-251`). It
first verifies a minted NATS user JWT carried in the `A2a-Caller-Jwt` header,
and a token that fails to parse or verify is logged and then treated as
absent (lines 155-194). The signature shows that the auth callout issued the
token, not that the publisher is the caller it names: nothing binds the token
to the message, and the gateway copies every inbound header onto the message
it delivers to the agent (`runtime/dispatch.rs:206` and `632-640`), so every
agent that receives a request holds a token it can attach to messages of its
own. With `A2A_GATEWAY_TRUST_CALLER_HEADERS` enabled (`constants.rs:71`), the
gateway then accepts a serialized `X-A2a-Spicedb-Principal` header and
finally a raw `X-A2a-Caller-Id`. When nothing resolves, the request still
proceeds: audit attribution becomes the `_` sentinel (`constants.rs:82`,
`jwt_caller_identity.rs:262-270`) and tier-1 policy evaluates that sentinel
as though it were a caller (`runtime/tier1.rs:38`).

a2a-bridge holds the HTTP listener in front of the gateway
(`rsworkspace/crates/a2a/a2a-bridge/src/main.rs:51-55`,
`inbound.rs:760-761`); a2a-gateway listens only on NATS, and `a2a-nats-http`
is a local adapter whose README places it outside that role. The bridge
copies the HTTP `x-a2a-caller-id` header verbatim into the NATS
`X-A2a-Caller-Id` header (`inbound.rs:585-597`). To obtain a minted JWT it
strips `Bearer ` from the caller's `Authorization` header and sends the
external token as plaintext JSON on the core NATS subject
`a2a.bridge.auth.callout.request` (`auth.rs:127-160`, the token at line
143). Nothing in this repository responds on that subject; the callout
subscribes only to `$SYS.REQ.USER.AUTH`
(`rsworkspace/crates/a2a/a2a-auth-callout/src/subscriber.rs:60`). Each
gateway request then rides a fresh connection that presents the minted JWT
as its token (`inbound.rs:214-238`) and carries it again in the
`A2a-Caller-Jwt` header (`inbound.rs:602-613`), so under
[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) it is
the callout that admits that connection and mints its allow list. That list
does not name the caller where it matters: the default caller grant publishes
on all of `a2a.v1.gateway.>` (`a2a-auth-callout/src/permissions.rs:60-70`),
and the ingress subject `{prefix}.v1.gateway.{agent_id}.{method}`
(`inbound.rs:51-58`) has no caller position.

Events repeat the pattern. The task event subject of
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) names only the
task, so the A2A server copies the caller headers from the request onto every
event (`rsworkspace/crates/a2a/a2a-nats/src/server/message_stream.rs:133-142`),
and the gateway's egress pump picks the caller-scoped fanout subject from
those headers (`a2a-gateway/src/gw_pull_backpressure.rs:788-799`). Whoever
publishes a task event decides which caller receives it.

The decider host binds one subject, `{prefix}.DeciderService.Decide`
(`rsworkspace/crates/decider/trogon-decider-nats-server/src/endpoint.rs:50`),
decodes each request before anything else (`runtime.rs:65-66`), and builds
every execution with neither a principal nor an authorizer
(`runtime.rs:182-187`). `WasmCommandExecution` defaults to
`WithoutAuthorization`
(`rsworkspace/crates/decider/trogon-decider-wasm-runtime/src/execution.rs:306`
and `342`), so any connection allowed to publish on that subject runs any
command. [ADR#0026](./0026-command-authorization-principal.md) shipped the
seam and left its ingress half unbuilt; the module documentation still says
that its Decision 4 mapping "is not implemented anywhere yet"
(`rsworkspace/crates/decider/trogon-decider-runtime/src/authorization.rs:18-25`).

AAuth exists, but it is off by default and it runs on a bus hop. The gateway
resolves `A2A_GATEWAY_AAUTH_MODE` to Off when it is unset
(`a2a-gateway/src/runtime/aauth_env.rs:192-203`). Off resolves every request
to an anonymous AAuth identity, and Shadow does the same for a request that
fails verification, so the request proceeds on whatever the header path
resolved (`aauth.rs:281-282` and `406-410`). When the layer is on, it
verifies the NATS proof-of-possession binding of
[ADR#0017](./0017-aauth-agent-authentication.md) on the gateway's NATS
ingress. Under
[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) the
import that carries a tenant's traffic into the platform account inserts the
account token into the subject, and
[ADR#0051](./0051-fully-bound-request-signing.md) Decision 6 records that
subject rewriting breaks signatures over subjects. Replay protection is the
process-local `InMemoryReplayStore` (`aauth.rs:226-228`,
`runtime/aauth_env.rs:137`), whose own documentation says a multi-replica
deployment accepts each nonce once per replica and names a shared NATS KV
backend, not yet built, as the intended replacement
(`rsworkspace/crates/aauth/trogon-aauth-verify/src/replay.rs:1-11`). The
replay key is the client-chosen nonce alone, `nats-pop:{nonce}`, with no key
id (`nats_pop.rs:222-225` and `275`). The default clock skew is 60 seconds in
the gateway (`a2a-gateway/src/constants.rs:166`) and 300 seconds in the
verifier (`nats_pop.rs:180`), against the 30 second ceiling of
[ADR#0051](./0051-fully-bound-request-signing.md) Decision 2. The verifier
crate already ships an RFC 9421 verifier, `HttpPopVerifier`
(`trogon-aauth-verify/src/http_pop.rs`, exported at `lib.rs:34`), and nothing
outside the crate uses it.

## Decision

### 1. Inside the bus the principal is derived from the subject, never from a header

A service on the bus learns on whose behalf a message was sent from the
subject the message arrived on, and from nothing else. The server enforces
the allow lists that the callout mints per connection
([ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md)),
and each caller's list admits a publish only on subjects that embed that
caller's own id. A subject that carries the tenant and the caller id is
therefore authenticated by construction: no caller's connection can publish
on a subject that names another caller.

Inside the platform account the tenant is the token that leads the subject,
the `{tenant}` of the
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) prefix: the
tenant account's public key, which the import inserts and the server
authorizes only for that account
([ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md)).
Where tenants share one account without imports, as single-account fixtures
do, the same position holds the tenant slug, which each caller's allow list
fixes to the caller's own tenant. Inside a tenant account the account itself
is the tenant, and its subjects carry no tenant token.

The caller sits at a position its subject grammar fixes: the grammar of
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) for gateway
ingress, agent delivery and task events, which gains that position, and
Decision 2 for the decider. A principal id that is not a single valid NATS
token is mapped to a stable, token-safe id when the callout mints the allow
list, so the id in the subject, the id in the grant and the id a receiver
derives are the same string.

A platform workload that relays (the gateway delivering to an agent, a
service forwarding a command to the decider) holds a grant that is a
wildcard over principal positions, because it relays for every caller. It
writes the principal of each outgoing subject only from the principal it
derived from the inbound subject that caused the publish, never from a
header and never from a payload. A callout-admitted workload that acts for
its own callers follows the same rule; the import pins its wildcard to its
own account token, so it can misattribute within its tenant and never
across tenants.

`X-A2a-Caller-Id`, and an `A2a-Caller-Jwt` forwarded by the bridge, are
routing hints at most. No service reads them as identity.

### 2. The decider host derives `CommandPrincipal` from the subject and fails closed

The decider subject carries the principal as the `PrincipalKind` wire name
(`agent`, `person` or `service`) followed by the `PrincipalId` of
[ADR#0026](./0026-command-authorization-principal.md), immediately before the
service and method:

```text
[{tenant}.]{prefix}.{kind}.{id}.DeciderService.Decide
```

`{prefix}` is the host's configured namespace of
[ADR#0057](./0057-decider-command-nats-binding.md), `decider` by default.
`{tenant}` is present wherever Decision 1 puts a tenant token; a client in a
tenant account omits it, and the import inserts it. From a generated client's
point of view any tenant token and the principal tokens are the head and the
tail of its configured prefix, so the terminal remains the method name and
the derivation of
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) is unchanged; a
client acting for a principal configures a prefix that ends in that
principal. The host subscribes with wildcards at the principal positions,
and at the tenant position when it serves several tenants.

For each delivery the host parses the kind token into `PrincipalKind` and the
id token into `PrincipalId` and builds the `CommandPrincipal` that
[ADR#0026](./0026-command-authorization-principal.md) Decision 4 maps from
the identity verified at the edge. It does this before `CommandRequest::parse`
(`runtime.rs:65-66`) decodes the request. A kind token that is not a wire
name, or an id token that `PrincipalId` rejects, is answered with
`UNAUTHENTICATED` and `PRINCIPAL_MISSING` (`status.rs:161`), and the payload
is never decoded. A subject without principal tokens matches no subscription
and reaches no host. The subject carries the kind and the id only, so the
derived principal has an empty claims set; an authorizer that needs roles or
scopes looks them up by principal rather than reading them from the message.
The `directed_principal` of
[ADR#0026](./0026-command-authorization-principal.md) Decision 4 stays the
untrusted hint that decision describes; it has no subject position.

The `authorization.rs` seam is mandatory in the host. Every execution is
built with `with_principal` and `with_authorizer`, never with
`WithoutAuthorization`. A deployment without its own policy gets the host's
default authorizer, which admits any derived principal and refuses an
execution that has none, as the trait's default `authorize_execution` does
(`authorization.rs:518-523`); a denial is `PERMISSION_DENIED` and
`PRINCIPAL_UNAUTHORIZED` (`status.rs:162`). Execution paths the host does not
serve, such as the `trogon-scheduler` worker processor that
[ADR#0026](./0026-command-authorization-principal.md) cites, are outside this
decision.

What this changes in [ADR#0026](./0026-command-authorization-principal.md):

- Decision 4. The mapping it describes runs where proof of possession is
  verified, at the external edge, and reaches the host as subject tokens; the
  host turns those tokens back into a `CommandPrincipal`.
- Decision 5. "With no authorizer configured, behavior is unchanged" no
  longer holds for the host, which always configures one.
- Consequences. "Enforcement is opt-in per call site" and "The seam ships;
  the ingress half does not" no longer hold for the host.

### 3. a2a-gateway has no anonymous path

The gateway derives its caller from the ingress subject under Decision 1 and
rejects the request, before any policy tier runs, when there is no caller to
derive. Identity material that accompanies a request is checked rather than
ignored: a forwarded `A2a-Caller-Jwt` that is malformed, expired, fails
verification, or names a caller other than the subject's is a rejection, not
a fallback. Nothing resolves to the `_` sentinel any more; audit, tracing and
tier-1 policy see a derived principal or never see the request.

`A2A_GATEWAY_TRUST_CALLER_HEADERS` is removed from the environment surface.
A test that needs a caller publishes on a subject that carries one; any
header shortcut that survives lives inside test fixtures and cannot be
reached from configuration.

### 4. AAuth proof of possession is enforced at the external HTTP edge, and only there

The external HTTP edge of a2a-gateway is the process that receives the
caller's HTTP request, which today is the a2a-bridge listener; the gateway
itself has none. The edge verifies AAuth proof of possession
([ADR#0017](./0017-aauth-agent-authentication.md),
[ADR#0051](./0051-fully-bound-request-signing.md)) with the RFC 9421 profile
that `HttpPopVerifier` implements before it asks for a mint, and the verified
identity becomes, through
[ADR#0026](./0026-command-authorization-principal.md) Decision 4, the
principal the callout mints into the caller's allow list.

Enforce is the default in release builds. Shadow and Off are accepted only
together with an explicit development flag that is separate from the mode
setting, and the edge logs a warning at startup whenever it runs unenforced.
Shadow verifies and logs as before, but it no longer admits a request as
anonymous: under Decision 3 the request still needs an identity, which in
Shadow and Off comes from the credential the callout verifies without proof
of possession.

Internal bus hops do not run AAuth, and neither does the first hop of a
tenant client that connects to NATS directly. Verification happens once, at
admission, as [ADR#0051](./0051-fully-bound-request-signing.md) Decision 6
prescribes, and the imports of
[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md)
rewrite every subject that crosses from a tenant account into the platform
account, which breaks a signature over that subject. A direct client is
admitted by the callout like any other tenant caller and takes its principal
from its subjects under Decision 1; the gateway verifies no per-message
signature from it. The gateway's NATS AAuth layer is retired, and the NATS
binding of [ADR#0017](./0017-aauth-agent-authentication.md) Decision 2 stays
specified with no hop running it.

Freshness and replay follow [ADR#0051](./0051-fully-bound-request-signing.md):

- Every request carries a client-generated `jti` covered by its signature.
  Where it rides on the wire is fixed by the edge verifier's profile, not
  here.
- The edge issues nonces in the manner of
  [ADR#0051](./0051-fully-bound-request-signing.md) Decision 3: it can answer
  any request with a fresh nonce challenge, which the client binds into its
  retry. Whether a surface demands one stays policy, as that decision leaves
  it.
- Token validity is at most 2 minutes, 1 minute by default, and clock skew
  and expiry leeway are tolerated up to 30 seconds.
- Replay records are keyed by key id and `jti`, checked and recorded with a
  single atomic conditional create, and admission fails closed when the store
  cannot be consulted.
- The store is the NATS KV bucket `AAUTH_REPLAY`, shared by every edge
  instance. It also holds the nonces the edge issues, so any instance honors
  a nonce another instance issued. It keeps one revision per key and a
  maximum age of 180 seconds, the record lifetime of
  [ADR#0051](./0051-fully-bound-request-signing.md): the 2 minute validity
  ceiling plus twice the 30 second skew.
- The tooling that provisions the accounts and platform users of
  [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md),
  the dev compose bootstrap included, creates the bucket in the platform
  account. The edge holds grants to read and create keys in it and nothing
  that creates or edits streams. At startup the edge checks that the bucket
  exists with that history and maximum age, and in Enforce it refuses to
  start otherwise.

What this changes in [ADR#0017](./0017-aauth-agent-authentication.md):

- Decision 2. No hop runs the NATS binding. Its client-chosen
  `AAuth-Sig-Nonce` is no longer a replay key; replay is keyed by key id and
  `jti`, and freshness can be demanded with a server-issued nonce.
- Decision 5. AAuth still runs before tier-1, tier-2 and tier-3 policy, but
  there is no JWT-header identity left for it to supersede. The identity it
  verifies reaches the gateway only as the subject principal, and which
  verified identity becomes that principal, and which stays a hint, follows
  [ADR#0026](./0026-command-authorization-principal.md) Decision 4.
- Decision 6. The mode no longer defaults to `off`: the edge defaults to
  `enforce` in release builds, and `shadow` or `off` needs the development
  flag and logs a warning. `shadow` no longer lets a request through as
  anonymous. The layer moves from the gateway's NATS ingress to the external
  HTTP edge, and its configuration moves with it. The 60 second leeway and
  skew defaults give way to the 30 second ceiling of
  [ADR#0051](./0051-fully-bound-request-signing.md).
- Consequences. The process-local `InMemoryReplayStore` gives way to the
  shared `AAUTH_REPLAY` bucket wherever Enforce runs.

### 5. External bearer tokens never travel over core NATS

The only credential the bridge puts on core NATS in readable form is the
minted NATS user JWT, which opens its per-request connection
(`inbound.rs:220-225`) and rides along as the `A2a-Caller-Jwt` hint
(`inbound.rs:602-613`). When the mint exchange has to carry the caller's
external credential, the bridge seals it to the callout's xkey so that only
the callout can read it; today the token travels in plaintext
(`auth.rs:143`). A deployment whose callout has no xkey has nothing to seal
to, so its exchange carries no external credential and can mint only the
identity the edge verified under Decision 4. The minted JWT is a credential
in its own right while it is valid, so apart from opening the connection it
goes only from the bridge to the gateway, and the gateway stops copying it
onto agent-bound messages.

Whether the production responder for `a2a.bridge.auth.callout.request` lives
in this repository is an open question. The rule is the same wherever it
lives.

### 6. Events and egress routing carry only the derived principal

A task event carries in its subject the principal of the request it answers,
which the agent copies from that request's subject; the layout belongs to
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md). The A2A server
stops copying `X-A2a-Caller-Id` and `X-A2a-Spicedb-Principal` onto events
(`message_stream.rs:133-142`). The egress pump reads the caller from the
event subject alone (`gw_pull_backpressure.rs:788-799`) and terminates an
event whose subject yields no caller, as it already terminates an event with
no routing (`gw_pull_backpressure.rs:607-621`). `Trogon-Req-Id` stays on the
event as correlation and never selects a caller. Every other reader of the
caller headers, such as the agent bridge that addresses push dead letters
from `X-A2a-Spicedb-Principal`, moves to the subject the same way, and the
headers are no longer written.

On the decider path, an event that records who commanded it records the
principal the host derived under Decision 2, which is the authority that
travels as provenance under
[ADR#0039](./0039-self-authenticating-event-provenance.md); no reader takes a
principal from a header there either. The principal is not a token of the
event stream subject, whose optimistic concurrency is per subject
(`rsworkspace/crates/decider/trogon-decider-nats/src/store.rs:350-357`).

Where a component holds a caller's credential and acts with it, as the bridge
does for an HTTP caller under Decision 5, the audit record it writes names
both the caller and itself as the presenter. Inside the bus a record names the
derived principal alone and carries the same `Trogon-Req-Id`, so a reader can
always tell a bridged request from a client that connected on its own, and no
record reads as if the caller held the connection when the bridge did.

## Alternatives Considered

### Keep identity in headers and verify a signed token in them

Rejected. A signed header token proves who issued it, not who sent the
message: the `A2a-Caller-Jwt` the gateway verifies today is such a token, and
every agent receives a copy. Binding the token to each message would mean
signing on every hop, which is the per-hop AAuth alternative and falls with
it.

### Run AAuth on every hop

Rejected. A relay publishes a new message, so it cannot carry the caller's
signature over it; per-hop signing would have each relay sign for the caller,
which is the relay asserting identity under another name.
[ADR#0051](./0051-fully-bound-request-signing.md) Decision 6 already verifies
once at admission, and the imports of
[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) break
a signature over any subject they rewrite.

### Verify AAuth at the gateway for clients that connect to NATS directly

Rejected. The import that carries such a client's publish into the platform
account rewrites its subject, and
[ADR#0051](./0051-fully-bound-request-signing.md) Decision 6 leaves subject
mapping unsupported in front of signed subjects. The callout-minted allow
list already fixes the client's principal in its subjects, and the TLS
handshake already proves possession of the key behind the client certificate
that [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md)
requires of every connection.

### Trust the gateway's headers downstream of the gateway

Rejected. Allow lists constrain subjects, not header values, so any
connection that may publish on a subject may set any header on it. The
gateway being trustworthy does not make the next publisher on the same
subject trustworthy. [ADR#0026](./0026-command-authorization-principal.md)
rejected a header as the carrier of the decider principal on the same
grounds.

### Derive the principal from the publishing connection

Rejected as the source a receiver reads. A relay publishes on its own
connection, so connection identity names the relay on every hop after
admission. Connection identity is what makes the subject trustworthy, through
the allow list, and the subject is what carries the principal across relays.

### Carry the principal in the decider request body

Rejected. The body is whatever the publisher wrote, exactly as a header is,
and [ADR#0057](./0057-decider-command-nats-binding.md) makes the request one
`DecideRequest`, which has no principal field.

### Put the principal after the method in the decider subject

Rejected. A token after the method breaks the derivation of
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md), where the
terminal is the method name, so generated clients could not reach the
endpoint.

### Let the edge create its replay bucket

Rejected, though services that own their buckets provision and validate them
today (`rsworkspace/crates/platform/trogon-gateway/src/credential/store.rs:151`
and `181`,
`rsworkspace/crates/decider/trogon-decider-nats/src/provision.rs:285`).
Creating a bucket needs stream-management rights, and the edge is the most
exposed process in the deployment. A bucket created by whichever instance
starts first also takes that instance's configuration; the provisioning
tooling creates it once and every edge checks it.

### Keep the in-memory replay store behind sticky routing

Rejected. Stickiness does not survive restarts, failover or rebalancing, so
each of those reopens the per-replica gap that `replay.rs:1-11` describes.

## Consequences

- Integration tests that publish to the decider
  (`rsworkspace/crates/decider/trogon-decider-nats-server/tests/serve.rs`,
  `module_store.rs`) need subjects with a derivable principal; a subject
  without principal tokens reaches no host.
- Gateway tests that expect a request without a caller to go through flip to
  rejection, such as `untrusted_caller_headers_cannot_change_audit_attribution`
  (`rsworkspace/crates/a2a/a2a-gateway/src/runtime/dispatch/tests.rs:83`),
  which today delivers the request and audits the `_` sentinel. The tests
  built on the sentinel (`runtime/tier1/tests.rs:62` and `78`, and
  `per_agent_allowlist_denies_anonymous_callers` in
  `a2a-gateway/tests/tier1_declarative_bundles.rs:173`) give way to a
  rejection test at ingress, since tier-1 never sees a missing caller.
  `shadow_and_disabled_authentication_preserve_delivery_and_audit_mode`
  (`runtime/dispatch/tests/aauth_tests.rs:63`) and
  `principal_present_overrides_even_anonymous_existing_identity`
  (`runtime/aauth_env/tests.rs:510`) go away with the gateway's AAuth layer,
  and the header-trust fixture (`runtime/dispatch/tests/fixture.rs:136`) and
  its policy test (`jwt_caller_identity/tests.rs:220`) go away with the
  variable.
- Tier-1 bundle rules that match the `user/_` sentinel to deny anonymous
  callers no longer fire, because the rejection happens before tier-1.
- The callout's default caller grant, `a2a.v1.gateway.>`
  (`permissions.rs:64`), narrows to subjects that carry the caller's own id.
- [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) and
  [ADR#0057](./0057-decider-command-nats-binding.md) are drafts; each takes
  the principal positions into its grammar before acceptance, and the startup
  subject check of [ADR#0057](./0057-decider-command-nats-binding.md)
  validates the host's subscription with them.
- Enforce requires the `AAUTH_REPLAY` bucket. The provisioning tooling of
  Decision 4 creates it in every deployment that runs the edge, the dev
  compose stack included; a test that runs an edge in Enforce creates it in
  its fixture; and an edge in Enforce does not start without it. Each
  admission costs a KV round trip, and an unavailable bucket stops admissions
  instead of degrading to unprotected ones.
- The [ADR#0049](./0049-revocation-latency-target.md) target applies to the
  replay window. A key revoked while its `jti` records or issued nonces are
  still within the 180 second window is refused once the edge observes the
  revocation, which [ADR#0049](./0049-revocation-latency-target.md) bounds at
  a 5 second p99 per edge replica and, when the event path fails, at its
  backstop of at most 330 seconds. The window bounds replay detection and
  never extends acceptance of a revoked key. For keys that an external issuer
  publishes, revocation reaches the edge no faster than its JWKS cache, the
  kind of federated boundary
  [ADR#0049](./0049-revocation-latency-target.md) already places outside its
  target.
- The seam hands the authorizer the principal and the command only
  (`authorization.rs:509`), so a host that serves several tenants authorizes
  without the tenant token it parsed. A policy that differs between tenants
  needs the tenant carried to the authorizer, which is not decided here.
- A workload whose grant is a wildcard over principal positions is trusted to
  write the principal it derived. For platform workloads that is the trust
  their credentials already carry; for a callout-admitted workload the
  account token confines it to its own tenant. Narrowing such a grant to the
  principals that actually called the workload is not decided here.
- `a2a-nats-http` publishes to the gateway under one configured caller JWT
  for every HTTP client
  (`rsworkspace/crates/a2a/a2a-nats-http/src/runtime.rs:74-85`). It stays a
  local adapter; exposing it to external parties makes it an external HTTP
  edge under Decision 4.
- `HttpPopVerifier` gains its first caller outside its crate, and the
  gateway's AAuth layer, its in-memory replay store and its anonymous
  resolutions are removed.

## References

- [ADR#0081: Every NATS Connection Uses TLS Unless Explicitly Disabled, and Clients Enter Operator-Mode Accounts Only Through Platform Credentials or the Auth Callout](./0081-nats-tls-by-default-and-operator-mode-admission.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [ADR#0016: Protocol Buffers RPC over NATS micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
- [ADR#0039: Self-Authenticating Event Provenance](./0039-self-authenticating-event-provenance.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)
- [ADR#0057: Decider Command NATS Binding](./0057-decider-command-nats-binding.md)
- RFC 9421 (HTTP Message Signatures); RFC 9449 (OAuth DPoP, server nonce)
