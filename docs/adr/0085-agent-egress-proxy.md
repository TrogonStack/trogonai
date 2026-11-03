---
number: "0085"
slug: agent-egress-proxy
status: draft
date: 2026-11-02
---

# ADR#0085: Agent Outbound HTTP(S) Goes Through a Dedicated Egress Proxy That Admits by Exact Destination and Intercepts TLS Only to Inject a Credential

## Context

Policy already says what an agent may reach. [ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
makes a network permission a policy binding on a subject where absence is
denial, and [ADR#0072](./0072-agents-improve-through-recorded-candidates.md)
Decision 3 names the first such binding, `policy/egress_permissions`.
[ADR#0063](./0063-agent-connection-declarations.md) Decision 5 adds that a
connection declaration grants nothing and that the component which mediates
egress is not defined. Nothing enforces any of this. The agent's outbound
calls are unmediated today: a process the platform launches opens whatever
connection its sandbox lets it open, and no component sits on that path to
look up a permit, hold a credential, or write down what happened.

What exists is the contract and nothing around it.
`proto/trogonai/policy/egress_permissions/v1alpha1` defines initialize, set
and clear commands, their events, and `EgressPermissionPolicy`: a non-empty
list of `EgressDestination`, whose only arm is `EgressHost { host, port }`.
A host is an exact name or address, never a pattern, so the message rejects
`*`, `/` and a space, and a port is 1 to 65535. The policy attaches to a
`PolicySubject`, one Agent or one hierarchy node, and permits accumulate
downward. The message has no deny arm, no wildcard, no method or path, no
rate or budget, and no field saying a destination needs TLS interception.
There are no Rust bindings: `proto/trogonai/policy` is in the exclude list of
`buf.gen.yaml`, so no decider, no projection and no evaluator for it exists.
The comments in that file also say that evaluation storage and rubric
storage are never permitted destinations for a Session under evaluation.

Two neighbors are close to this problem and neither fits.
`trogon-provider-gateway` is described in its README as the HTTP foundation
for provider egress, with no routes yet. [ADR#0032](./0032-model-route-and-credential-binding.md)
makes it the model access service: it projects one admitted route per
selection, with no substitution and no failover, and it resolves the
provider credential itself. Reaching any other host is a different job with
different semantics, and giving the same process a general forward-proxy mode
would put route-pinned behavior and open-ended destinations behind one
listener. The A2A gateway carries agent-to-agent messages over NATS and has no
HTTP forward path.

Credentials are the second reason. Provider credentials already stay out of
the agent ([ADR#0032](./0032-model-route-and-credential-binding.md) Decision
3), and [ADR#0063](./0063-agent-connection-declarations.md) records that the
value must never enter the agent's address space. For non-model hosts there is
nowhere to apply that rule except at the point where the request leaves, and
only a component that can read the request can set a header on it.

Identity and freshness have constraints from the rest of the platform. An
agent is identified by an `AgentId` derived from a public key
([ADR#0036](./0036-agent-self-certifying-identity.md)), and the egress
permit must be looked up under that identity, not under a header the agent
wrote ([ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
makes the same argument for the bus). A revoked permit has to stop taking
effect within the bound of [ADR#0049](./0049-revocation-latency-target.md): a
5 second p99 target and a 330 second hard ceiling. Rustls with aws-lc-rs is
the TLS stack ([ADR#0015](./0015-rust-tls-library.md)), and the secrets
service is the only component that talks to OpenBao and answers over NATS core
request/reply ([ADR#0023](./0023-secret-management-and-key-custody-direction.md)
Decision 3).

## Decision

### 1. Agent outbound HTTP(S) goes through a dedicated egress proxy, in its own process

A new crate, `trogon-egress-proxy`, runs as its own process and is the
enforcement point for `policy/egress_permissions`. It is not a mode of
`trogon-provider-gateway`, which stays the model access service of
[ADR#0032](./0032-model-route-and-credential-binding.md): the model path
(implementation, supervisor, model access service) is unchanged and is not
routed through the egress proxy. It is also not an Envoy deployment with an
adapter in front of it. The admission logic is written behind the evaluation
seam of Decision 6 so that an external-authorization adapter can call the same
function if a later ADR adopts one.

The code that mints leaf certificates for Decision 2 lives in a second crate,
`trogon-egress-intercept`, so the CA key handling is isolated from the
listener. [ADR#0034](./0034-rust-crate-domain-grouping.md) gets an `egress`
group row for the two.

### 2. The proxy is an HTTP CONNECT forward proxy, SNI-only by default, and intercepts TLS only to inject a credential

An agent is configured with `HTTPS_PROXY` and reaches the world with `CONNECT
host:port`. The proxy admits or refuses the tunnel on the exact `host:port` of
the CONNECT target (Decision 4). It does not terminate TLS by default. After
the tunnel opens it reads the TLS ClientHello and requires the SNI to equal
the CONNECT host, so a client cannot ask for one permitted host and speak to
another (domain fronting). A mismatch, or a first record that is not a
ClientHello, closes the tunnel and is audited. A tunnel to a non-TLS port has
no SNI to compare, and the exact permit alone decides it.

TLS is intercepted, terminated and re-originated, for exactly one reason and
exactly one set of hosts: a destination that has a credential binding. For
those hosts only, the proxy presents a leaf certificate minted by a
per-deployment interception CA, reads the request, asks the secrets service for
the credential, sets it in the position the binding names, and sends the
request upstream over a fresh rustls connection that verifies the upstream
certificate ([ADR#0015](./0015-rust-tls-library.md)). The agent never holds the
credential. Whether a host is intercepted is computed from the binding set
into the policy snapshot; it is not a field of `EgressPermissionPolicy`, and a
binding by itself grants no reachability: the egress permit must also admit
the destination, so interception can only narrow what the permit allows.

The interception CA's private key is custody material under
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) and
[ADR#0033](./0033-key-custody-product-model.md). It stays in the proxy
process, and the leaf minting code refuses a host outside the current
interception set. The binding model, the placeholder format, and response
scrubbing are the subject of a later ADR; this one fixes the contract with the
secrets service.

The proxy resolves a credential by NATS core request/reply to the secrets
service, never from JetStream, with the tenant derived from the authenticated
NATS connection and not from the request ([ADR#0023](./0023-secret-management-and-key-custody-direction.md)
Decision 3). The service applies the credential's release policy. The proxy
may cache a resolved value for at most 330 seconds with jitter and evicts it on
the service's invalidation event, the bound of
[ADR#0049](./0049-revocation-latency-target.md). The rule is fail closed:

- no credential, no send. When a binding exists and the credential is missing,
  revoked, not releasable, or the service is slow, unreachable or has no
  responder, the request is not forwarded;
- the proxy never falls back to another binding, another credential, or a
  value the agent supplied, and it removes whatever the agent put in the
  credential's position;
- the proxy never answers 401, because a 401 invites the agent to retry with a
  credential of its own.

### 3. Caller identity at egress is an mTLS client certificate whose key the agent code does not hold

The proxy listener requires a client certificate. The key behind it is held by
the supervisor or the sandbox host and never by the agent code, because an
agent that could read the key could present the identity of any other agent it
could name. Most HTTP clients cannot present a client certificate to a proxy,
so the agent speaks plain CONNECT to a supervisor-side forwarder that is
reachable only from inside the sandbox, and the forwarder opens the mTLS
connection to the proxy with the certificate the agent cannot read.

The certificate binds the `AgentId` ([ADR#0036](./0036-agent-self-certifying-identity.md))
in exactly one URI subject alternative name, and the proxy takes the caller
from nothing else: not the subject DN, not a header, not a field in the
CONNECT request. A certificate that does not chain to a configured anchor, is
expired or not yet valid, or has no single URI SAN fails the handshake. The
URI is a SPIFFE-style identifier, a derived alias of the `AgentId` and never an
independent name: the same `AgentId` always yields the same alias, and
verification maps the alias back before any lookup. The certificates are
issued by our own CA over NATS, under the connection admission of
[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md), not by
SPIRE. That is a direction; the
certificate profile, the lifetime, and the issuance subjects belong to a later
ADR. The certificate also names the Session, so the audit trail and the
evaluation-storage exclusion of
[ADR#0072](./0072-agents-improve-through-recorded-candidates.md) can be
applied to the Session that made the call.

### 4. Policy is a per-agent allowlist of exact destinations, evaluated against a bounded-stale projection

The proxy admits a connection when the destination equals a permitted
destination of the calling agent. The permitted set of an agent is the union of
its own `egress_permissions` binding and those of every ancestor node
([ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
Decision 2), computed by a projector and written to a NATS KV bucket that the
proxy watches. Nothing else widens it: not a connection declaration, not a
binding, not a header. There are no deny lists and no precedence between
rules ([ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)),
so a refusal is always one missing permit or one hold, and the audit record
names it. The default is deny, and the proxy follows
[ADR#0084](./0084-policy-enforcement-posture.md): it enforces by default and
refuses to start when no policy source is configured. Evaluation storage and
rubric storage stay blocked for a Session under evaluation whatever the set
says.

[ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
Decision 7 says evaluation is live. For this data plane, live means the
proxy evaluates against its own projection and that the projection's
staleness is bounded by [ADR#0049](./0049-revocation-latency-target.md). The
projector writes a heartbeat key on a short interval, and the proxy measures
snapshot age from the heartbeats it has seen through its own watch, because a
quiet bucket does not change and an entry's age proves nothing. The age bound is
configurable and defaults to 30 seconds. It is never above the 330 second
ceiling. Past the bound the proxy refuses every new connection, and it closes
open tunnels it can no longer vouch for. When a permit is revoked, the
tunnels the proxy admitted under it are closed. A proxy with no snapshot asks
the policy service once over NATS request/reply and denies on timeout. Each
decision records the permits and the projection revision it evaluated, in the
form of `PolicyRuleRef`, so a past decision can be reconstructed from the
policy streams.

### 5. Refusals carry a status the agent can act on and a machine-readable reason

The proxy answers a refused CONNECT with an HTTP status and the header
`x-trogon-egress-fault: <CODE>`, plus a short body that names the code and
never the contents of the policy.

| Condition | Status | Examples of `CODE` |
| --- | --- | --- |
| Policy denies: no permit, a forbidden address, an SNI that does not match, a blocked evaluation target, a credential that exists but cannot be released | 403 | `NO_PERMIT`, `ADDRESS_FORBIDDEN`, `SNI_MISMATCH`, `EVALUATION_TARGET_BLOCKED`, `RELEASE_DENIED` |
| The policy snapshot is stale or unavailable, the secrets service is unavailable, or the audit queue is full | 503 with `Retry-After` | `POLICY_STALE`, `POLICY_UNAVAILABLE`, `SECRETS_UNAVAILABLE`, `AUDIT_UNAVAILABLE` |
| A rate or budget refusal | 429 | reserved, not emitted by this record |

Operational unavailability is never reported as a security denial
([ADR#0079](./0079-external-secret-sources.md) Decision 5), so an agent and an
operator can tell "you may not" from "we cannot decide right now". A missing or
invalid client certificate fails the TLS handshake before any HTTP exists, so
that cause is audited and cannot be returned as a status. A throttled secrets
service is reported as 503 until a later ADR defines the 429 contract.

Every decision, allow and deny, emits one audit event with the decision
identifier, tenant, agent, Session, destination, the `PolicyRuleRef` list and
projection revision, the TLS mode (passthrough or intercepted), and the fault
code, and when a credential was injected, the binding and credential
identifiers. No event carries a header value, a body, or a credential. The
audit envelope and its stream are a later ADR. When the proxy cannot emit an
event it refuses new connections with `AUDIT_UNAVAILABLE`, and a deny stands
even when its event cannot be published.

### 6. Start with a static snapshot behind the same evaluation seam

The first version loads the allowlist, and the interception set, from a
file. The proxy reads both through one evaluation interface, `admit(identity,
destination, now) -> Permit or Refuse(fault)`, over a snapshot trait, so the
KV projection of Decision 4 replaces the file without touching the listener,
the interception path, or the audit path. A static snapshot is operator-authored,
does not come from the `egress_permissions` history, and is not a system of
record; the audit record names the snapshot source, and a deployment on the
static source cannot claim live revocation. The proxy follows
[ADR#0084](./0084-policy-enforcement-posture.md) for the static source too:
an empty or unreadable file means no policy loaded, which means deny, and
which means the process does not start.

### 7. Destinations resolve to addresses the policy intended

The proxy treats a destination as routing data that can be steered
([ADR#0032](./0032-model-route-and-credential-binding.md) Decision 8):

- an IP literal as the CONNECT host is denied unless the permit is that exact
  literal;
- the proxy resolves the name itself, connects only to the address it
  validated, and does not resolve again, so a rebinding answer between check
  and connect has no effect;
- a resolved loopback, link-local (including the metadata address), RFC 1918,
  or unique-local address is refused unless the permit is that literal, which
  keeps platform-internal services unreachable unless an operator permits
  them; and
- a redirect from an intercepted host is a new request and is admitted again
  against the destination it names.

## Alternatives Considered

### Add a forward-proxy mode to `trogon-provider-gateway`

Rejected. [ADR#0032](./0032-model-route-and-credential-binding.md) gives that
gateway a narrow contract: one admitted route per selection, no substitution,
no failover, a credential it resolves for the provider it serves. A forward
proxy admits an open-ended set of destinations and substitutes nothing. One
listener holding both would carry the weaker invariants of the second into the
first, and a bug in destination parsing would sit next to the process that
holds provider credentials. The two share a vocabulary for credential
binding, not a process.

### Envoy with an external-authorization adapter

Rejected for now. Envoy would give mature HTTP and TLS handling, but the
decisions that matter here (the mTLS identity mapped to an `AgentId`, the
bounded-stale projection, the fail-closed audit, the per-host interception
decision, the secrets-service round trip) all live in the adapter, so the
adapter is the product and Envoy becomes a second thing to configure, upgrade
and secure in front of it. A CONNECT proxy with SNI inspection is a small
amount of code. This is revisited if the admission function is later wanted
behind an external-authorization extension point, which Decision 1 keeps
possible.

### Transparent interception of all TLS

Rejected. Terminating every connection would give the proxy request-level
visibility everywhere, at the price of a CA that every agent must trust for
every host, a plaintext copy of all traffic inside the proxy, and breakage for
clients that pin certificates. Admission by exact destination does not need
plaintext, so the proxy takes plaintext only where it must: to set a
credential the agent must not hold.

### SPIRE for workload identity

Rejected as a starting point. SPIRE would give attested workload identities
and rotation, but it adds a server, agents and a node-attestation story to
operate, and the identity we need is already anchored in the `AgentId`, which
the platform issues certificates for over NATS. The SPIFFE identifier form is
kept as an alias so a future move to SPIRE does not change what the proxy
trusts.

## Consequences

- There is a new process to operate, with its own availability, scaling and
  upgrade path. An agent that cannot reach the proxy has no egress, which is
  the intended failure and also an outage mode: a stale or unreachable policy
  source ends all agent network access once the bound passes.
- The proxy enforces only if nothing else leaves the sandbox. Without a sandbox
  that blocks every route except the supervisor-side forwarder, the proxy is
  advisory, and a Session must not claim enforced egress. Building that
  confinement and its start-up check is a separate piece of work.
- Agents must be started with `HTTPS_PROXY` and, for intercepted hosts, the
  interception CA in their trust store. A client that ignores `HTTPS_PROXY`,
  speaks raw sockets, UDP or QUIC, or pins certificates is not mediated by
  this proxy and is stopped only by the confinement above. A pinned host cannot
  be intercepted and so cannot have a credential injected.
- Because `EgressHost` is exact, a service that fronts many hostnames needs
  each one enumerated. A deny-by-default allowlist with no patterns is the
  cost of the "relabel cannot widen a permission" property.
- SNI is client-supplied and TLS 1.3 encrypted ClientHello would hide it. The
  proxy refuses a ClientHello whose SNI it cannot read, so a client using
  encrypted SNI cannot pass an SNI-only check.
- The sandbox, the proxy, and the supervisor-side forwarder now form a trusted
  computing base for egress. The interception CA key and the plaintext of
  intercepted requests exist inside the proxy process, and a compromise of
  that process is a compromise of every injected credential in its cache.
- Dropping tunnels when the snapshot passes the bound means a long-lived
  stream can be cut by a policy-source outage of more than the bound. This is
  chosen over a partitioned proxy keeping a revoked agent connected.
- The static snapshot is a stage, not an end state. Until the projection and
  the `egress_permissions` bindings exist, policy changes are file changes
  with no history.
- The Rust bindings for `policy` must be generated, and a decider and a
  projector built, before the KV-backed source exists.
  [ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
  Decision 7 should state that "live" means the bounded projection of Decision
  4 here.
- [ADR#0063](./0063-agent-connection-declarations.md) Decision 5 can point to
  this record for the egress point. The grant and the connection resource it
  leaves open are still open.
- Later ADRs own the certificate profile, the credential binding model and
  placeholder, the audit envelope, and the 429 contract.

## References

- [ADR#0015: Rust TLS Library](./0015-rust-tls-library.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0033: Key Custody Product Model](./0033-key-custody-product-model.md)
- [ADR#0034: Rust Crate Domain Grouping](./0034-rust-crate-domain-grouping.md)
- [ADR#0036: Agent Self-Certifying Cryptographic Identity](./0036-agent-self-certifying-identity.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0063: Agent Connection Declarations](./0063-agent-connection-declarations.md)
- [ADR#0071: What an Agent May Reach, How Far It May Go, and Whether It May Run at All Are Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
- [ADR#0072: Agents Improve by Running Recorded Candidates, Scored Blind, and Promoted Only Through Proposals](./0072-agents-improve-through-recorded-candidates.md)
- [ADR#0079: External Secret Sources Resolve Behind the Secrets Service](./0079-external-secret-sources.md)
- [ADR#0081: Every NATS Connection Uses TLS Unless Explicitly Disabled, and Clients Enter Operator-Mode Accounts Only Through Platform Credentials or the Auth Callout](./0081-nats-tls-by-default-and-operator-mode-admission.md)
- [ADR#0082: Inside the Bus the Principal Is Derived from the Subject, and Proof of Possession Is Enforced Only at the External Edge](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
- [ADR#0084: Every Policy Enforcement Point Enforces by Default and Refuses to Start Unenforced](./0084-policy-enforcement-posture.md)
- [HTTP Semantics, CONNECT method (RFC 9110, section 9.3.6)](https://www.rfc-editor.org/rfc/rfc9110#section-9.3.6)
- [SPIFFE ID specification](https://github.com/spiffe/spiffe/blob/main/standards/SPIFFE-ID.md)
