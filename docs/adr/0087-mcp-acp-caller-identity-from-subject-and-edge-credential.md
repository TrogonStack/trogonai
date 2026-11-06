---
number: "0087"
slug: mcp-acp-caller-identity-from-subject-and-edge-credential
status: draft
date: 2026-11-06
---

# ADR#0087: MCP and ACP Callers Are Known the Same Way A2A Callers Are: the Principal Comes from the Subject, and a Verified Credential Is Required Only at Each Server's HTTP Edge

## Context

[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
decides, for A2A, that inside the bus the principal is the `{kind}.{id}` pair
in the subject, and that a credential is verified only where HTTP meets the
bus. It names no other protocol. `mcp-nats-server` and `acp-nats-server` are
two more HTTP listeners in front of the same bus, and neither has either half
of that rule.

`mcp-nats-server` serves `/mcp` with no authorization layer
(`rsworkspace/crates/mcp/mcp-nats-server/src/main.rs:43-48`). The only gate is
rmcp's allowed-host list (`runtime.rs:44-56`), which guards against DNS
rebinding and says nothing about who the caller is. The process opens one NATS
connection (`main.rs:39`) and every HTTP session publishes through it under a
generated client id. `Authorization` never reaches the bus in any case:
[ADR#0041](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md) forwards only
`MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` and `Mcp-Param-*`
(`rsworkspace/crates/mcp/mcp-nats/src/mcp_transport_headers.rs:19` and `72`).
[ADR#0094](./0094-oauth-authorization-protocol.md) records the missing
authorization and states that inbound MCP needs its own ADR. This is that ADR.

`acp-nats-server` has the same shape. It opens one shared NATS connection
(`rsworkspace/crates/acp/acp-nats-server/src/main.rs:37`) and builds one
`Bridge` per HTTP or WebSocket connection over it (`component.rs:92`). Its
layers are SSE buffering, protocol-version tracking and an Origin check
(`compat.rs`, `apply_layers`). The Origin check passes a request that carries
no Origin header (`origin_rejection`, `compat.rs:135-139`), so every non-browser
client passes it; it keeps browsers out and does not identify anyone. Both
servers bind loopback by default, which limits who can reach them and does not
remove the question for any local process.

Behind the listeners the subjects name no caller. MCP uses
`{prefix}.v1.server.{server_id}.{method...}` toward the server, for example
`mcp.v1.server.{server_id}.tools.call`, and
`{prefix}.v1.client.{client_id}.{method...}` toward the client. ACP uses
`{prefix}.v1.session.{sid}.agent.{method}` and `...session.{sid}.client.{...}`
for session traffic and `{prefix}.v1.global.agent.{method}` for `initialize`,
`authenticate` and `session.new`
(`rsworkspace/crates/acp/acp-nats/src/nats/subjects/`). A responder that must
decide whether the sender may act can only read a header, and
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
Decision 1 explains why a header cannot carry identity. For ACP the
consequence is concrete: any publisher that knows or guesses a session id can
send `prompt` to a session it did not create, and the agent has no way to tell.

Both crates also find the structure of a subject by searching for markers.
`parse_role_subject` looks for `.server.` or `.client.` anywhere in the subject
and reads what follows (`rsworkspace/crates/mcp/mcp-nats/src/nats/parsing.rs:135`),
and the ACP parser uses `rfind(".agent.")` and a search for the session marker
(`rsworkspace/crates/acp/acp-nats/src/nats/parsing.rs:141` and `147`). Once a
caller-chosen id sits in the subject, a marker search can be steered by an id
that equals a marker.

The A2A path has moved past the state ADR 0082 describes. `a2a-bridge` verifies
an AAuth proof at its HTTP edge in `Enforce` mode by default, with a separate
development flag required for `Shadow` or `Off`
(`rsworkspace/crates/a2a/a2a-bridge/src/constants.rs:22` and `26`,
`edge_auth.rs`). It requires the proof's presenter to equal the subject of the
caller's bearer token, seals the bearer to the callout's key, and asks the
callout to mint a NATS user for that caller (`auth.rs`). The callout verifies
the bearer, derives the principal, and issues a `CallerGrant` that confines the
user to that principal's subjects
(`rsworkspace/crates/a2a/a2a-auth-callout/src/permissions/caller_grant.rs`).
The bridge connects as that user and publishes with the principal in the
subject (`inbound.rs`). MCP and ACP can reuse this path; they cannot reuse it
as written, because `CallerGrant` is built from the A2A prefix and subject types
only.

Two lifetimes disagree. A minted user is valid for
`AUTH_CALLOUT_USER_JWT_TTL_SECS`, 300 seconds unless configured
(`devops/nats/README.md`, `constants.rs:32` in the callout), and NATS closes a
connection whose user JWT has expired. An MCP session or an ACP connection can
last for hours.

The [policy enforcement posture proposal](https://github.com/TrogonStack/trogonai/pull/745)
(in review as a draft ADR) says every enforcement point, including the MCP
boundary, ships in `Enforce` and refuses to start unenforced. This ADR is
written to be consistent with it.

## Decision

### 1. Each server's HTTP layer is the external edge for its protocol, and it admits a caller only with a verified credential

`mcp-nats-server` and `acp-nats-server` are external HTTP edges in the sense of
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
Decision 4. Each one:

- admits a request only when the credential defined for its protocol in
  Decisions 2 and 3 is present and verified. There is no anonymous path and no
  default principal; a request that cannot be attributed is refused with 401
  before anything is published;
- starts in `Enforce` by default and refuses to start with admission off,
  following the
  [policy enforcement posture proposal](https://github.com/TrogonStack/trogonai/pull/745).
  A non-enforcing mode (`Shadow` or `Off`) is reachable only through a
  separate development flag, the same arrangement as the A2A bridge. If that
  proposal changes, this Decision follows it;
- does not treat loopback binding, the allowed-host list or the Origin check as
  authentication. They stay as defenses against rebinding and browsers;
- never forwards the credential onto the bus
  ([ADR#0041](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md)). The bearer
  goes to the callout sealed to its key, as in
  [ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
  Decision 5, and the edge keeps no copy of the bearer beyond the request that
  carried it;
- never lets the edge be the verifier of the bearer. The edge may read the
  bearer's subject unverified to compare it with an AAuth presenter, exactly as
  the A2A bridge does, but the principal is the one the callout derives from a
  token the callout verified.

Local stdio bridges (`mcp-nats-stdio`, `acp-nats-stdio`) are not edges. They
connect to NATS directly and are admitted by the callout like any other direct
caller under [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md).

### 2. The MCP edge accepts an OAuth bearer access token and verifies a signed proof when the request carries one

MCP clients that follow the pinned authorization specification (revision
2026-07-28, as pinned in
[ADR#0094](./0094-oauth-authorization-protocol.md)) send
`Authorization: Bearer` on every HTTP request and discover the authorization
server from RFC 9728 protected resource metadata. That revision makes
authorization optional overall, asks HTTP transports to conform to it, and
makes the listed server behaviors mandatory for a server that implements it.
This repository's servers are public-facing edges and implement it, so the
MUSTs apply. The edge therefore:

- serves RFC 9728 protected resource metadata for the MCP endpoint and answers
  an unauthenticated or invalid request with 401 and a `WWW-Authenticate:
  Bearer resource_metadata="..."` challenge. It answers 403 for a valid token
  with insufficient scope;
- accepts a bearer access token as the credential and sends it, sealed, to the
  callout, which verifies it with its OIDC verifier. That verifier honors the
  limits it has today: one configured issuer, JWT access tokens only, RSA
  signatures only, a `kid` that resolves to a published verification key. A
  client whose authorization server is another issuer, or whose token is
  opaque, is refused until the verifier changes. The protected resource
  metadata advertises the configured issuer and nothing else;
- requires the token's audience to equal the canonical URI of the MCP resource
  (RFC 8707). The callout therefore needs an audience set per surface; a token
  issued for the A2A bridge must not be accepted by the MCP edge. The verifier's
  audience list today is shaped for ID tokens and is the part that changes;
- never accepts or passes through a token issued for another resource, and
  never uses the client's token to call anything else;
- verifies AAuth proof of possession (RFC 9421 signatures, `AAUTH_REPLAY`
  replay protection, the 30 second skew ceiling of
  [ADR#0051](./0051-fully-bound-request-signing.md)) whenever the request
  carries signature headers, with the same `HttpPopVerifier` the A2A bridge
  uses and the same binding rule: the presenter must equal the bearer's
  subject. A proof that fails verification refuses the request. It never falls
  back to bearer-only admission;
- prefers the proof where both are possible, and records in the audit trail
  whether a request was admitted with a verified proof or on the bearer alone.

Bearer-only admission is the compatibility tier of
[ADR#0050](./0050-signed-first-caller-authentication.md) Decision 1, "the
explicitly labeled compatibility tier for tooling that cannot sign requests,
not a peer option", and it stays subject to the policy bound of Decision 5 of
that ADR: a deployment can turn it off for a server, and then only signed
requests are admitted. ADR 0050 is written about platform API keys, so this is
an application by analogy to an OAuth access token, not a restatement of what
that ADR says about tokens. The MCP specification gives stock MCP clients no
way to sign requests, so refusing bearer outright would make the server
unusable by the clients it exists to serve. A deployment that can require
signing for its own clients should.

### 3. The ACP edge accepts the same bearer only together with a bound AAuth proof

The ACP edge uses the credential the A2A bridge uses: a bearer token that the
callout verifies, plus an AAuth proof whose presenter equals the bearer's
subject. There is no bearer-only tier.

The reason is the transport. ACP's streamable HTTP is POST, GET and DELETE on
`/acp`, and every request can carry signature headers. Its WebSocket upgrade is
an HTTP request the client controls, and browser upgrades are refused outright
because the transport rejects any upgrade that carries an Origin, so a
WebSocket client is not a browser and can sign its upgrade. Frames after the
upgrade carry no per-frame credential; the upgrade request authenticates the
connection (see Decision 7). Nothing in the transport stops a client from
signing, so the compatibility tier has no justification here.

If a real ACP client is found that cannot sign, bearer-only admission for ACP is
decided in an amendment that names that client. It does not arrive by default.

### 4. The principal enters the bus through a per-caller connection minted by the callout, in a fixed subject position, never through a header

After admission the edge asks the callout to mint a NATS user for the caller and
connects with that user, one connection per HTTP session (MCP) or per ACP
connection id. It publishes on that connection. The edge's own process
credential holds no grant to publish on principal positions.

The principal is written into the subject at a fixed position, the place
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) gives it, after the
bounded entity token and before the role or method:

| Surface | Subject |
| --- | --- |
| MCP, to the server | `mcp.v1.server.{server_id}.{kind}.{id}.{method...}` |
| MCP, to the client | `mcp.v1.client.{client_id}.{kind}.{id}.{method...}` |
| ACP, session, to the agent | `acp.v1.session.{sid}.{kind}.{id}.agent.{method}` |
| ACP, session, to the client | `acp.v1.session.{sid}.{kind}.{id}.client.{...}` |
| ACP, global | `acp.v1.global.{kind}.{id}.agent.{method}` |

`{kind}.{id}` is the `SubjectPrincipal` of
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md),
with the same id rules, and it spends two of the 16 tokens
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) allows. The longest
ACP session subject today stays well under the limit.

The global ACP subjects get a principal position as well. `session.new` is the
subject on which the agent first learns who is creating a session (Decision 5),
so it cannot stay anonymous.

Responders (MCP servers behind `server_id`, ACP agents) refuse a subject whose
principal tokens are missing, malformed or in digest form from a credential,
with the parsing `SubjectPrincipal::from_tokens` already provides. They copy the
principal from the inbound subject onto each response, event and
client-direction request they cause, so a client-direction request such as
sampling or `fs` reaches the one caller it belongs to. They hold wildcard grants
over principal positions as relays and are trusted only to copy it, which is
the trust model ADR 0082 already accepts. An edge user subscribes only to
subjects carrying its own principal.

Parsers read fixed positions. Both crates drop their marker searches: the role
is the token after the entity id and the principal, not whatever the first
`.server.` happens to be. This is required, not tidiness: after this change an
id chosen by a caller sits earlier in the subject than the role.

The HTTP session (MCP session, ACP connection id) is bound to the principal
that opened it. A later request on the session that admits as a different
principal is refused. This closes the case where one caller's session
identifier is replayed by another.

ACP subjects are captured by five JetStream streams, `COMMANDS`, `RESPONSES`,
`CLIENT_OPS`, `GLOBAL` and `GLOBAL_EXT`, each with an enumerated list of
subjects (`rsworkspace/crates/acp/acp-nats/src/nats/subjects/`), and stored
messages are filed under the old subjects. Changing the shape needs the stream
subject lists and the consumers' filters
(`jetstream/consumers.rs:18`) to change and the existing stored messages to be
moved or retired. That migration is not designed here (see Not decided here); it
is a prerequisite of shipping this Decision for ACP and must not change the
shape above.

### 5. An ACP agent binds a session to the principal that created it, and refuses commands from another principal

> This Decision is new behavior for the ACP agent and the one most open to
> revision. It is separable from Decisions 1 to 4: they stand without it.

When the agent handles `session.new`, it records the principal taken from the
subject as the session's owner. Every later session-scoped command, `prompt`,
`cancel`, `load`, `fork`, `resume`, `close`, `delete`, `set_mode` and
`set_config_option`, is accepted only when the principal in its subject is the
owner. A refusal is indistinguishable from the session not existing, so a
caller cannot probe for other callers' session ids. `list_sessions` returns
only the principal's own sessions. A `fork` creates a new session owned by the
forking principal, and only the owner may fork.

The check lives in the agent, in the `acp-nats-agent` framework, not in the
edge. The session state is there, and the edge is not the only publisher: a
local bridge or another edge reaches the same agent with the same session id.
The agent can only tell them apart by the principal in the subject, which
Decision 4 makes unforgeable inside the bus.

Sharing a session between principals (handing a session to a colleague) is not
provided here. It would be an explicit grant recorded on the session, not a
relaxation of this rule.

### 6. The callout mints per-surface grants, and shared grant code does not stay in an A2A-named crate

The callout builds a grant for each surface from that surface's own subject
types, as `CallerGrant` does for A2A:

- an MCP caller may publish only `mcp.v1.server.*.{kind}.{id}.>`, subscribe
  only to `mcp.v1.client.*.{kind}.{id}.>`, and use an `_INBOX.{kind}.{id}`
  reply prefix;
- an ACP caller may publish only the command methods of its own principal
  under `acp.v1.session.*.{kind}.{id}.agent.` and
  `acp.v1.global.{kind}.{id}.agent.`, and subscribe only to the response and
  client-direction subjects carrying its own principal. The grant lists methods
  from the subject types the parsers use, so a caller cannot publish a
  response or `cancelled` subject;
- an A2A caller keeps the grant it has.

The surface is derived from the mint subject on which the callout received the
request, with a mint subject per surface following the existing A2A one. It is
never read from the request body, the same rule as the principal. Without it a
caller admitted for one surface could ask for a grant on another.

Because no minted user holds a grant outside its own principal, a caller that
holds a user for one principal cannot publish as another. That is the whole
guarantee this record adds to the bus; it does not depend on the edge being
correct.

`CallerGrant` is tied to `A2aPrefix` and the `a2a_nats` subject types, and the
pattern and permission types around it (`SubjectPattern`, `IssuedPermissions`,
`AdmissionRoute`, grant composition) are not A2A specific. Under
[ADR#0034](./0034-rust-crate-domain-grouping.md) a crate that serves three
protocols is cross-cutting and belongs in `platform/`, not in the `a2a/` group.
The shared grant code and the callout move to `platform/`; `mcp-nats` and
`acp-nats` each own their grant-shaped subject types, as `a2a-nats` does, and
the dependency direction of [ADR#0002](./0002-rust-crate-boundaries.md) is kept
by having the callout depend on the protocol crates, never the reverse. Crate
names and the order of the moves are left to the implementation.

### 7. A minted user is renewed before it expires, and a session that cannot be renewed is ended

The edge tracks the expiry of the user it holds for each session. Before
publishing on a connection whose user JWT is within a configured margin of
expiry, the edge mints again from the credential of the request being served
and reconnects on the new user. The MCP specification requires a token on every
HTTP request, and an ACP HTTP request carries its own signature headers, so
each request has a fresh credential to present.

- The new user must name the same principal as the old one. A renewal that
  names a different principal ends the session.
- A failed renewal, whether the token has expired, the callout refuses, or the
  callout is unreachable, refuses the request with 401 and publishes nothing on
  the old connection. A user whose JWT has expired is never reused.
- A WebSocket has no per-frame credential. When its user expires and cannot be
  renewed from a request the client sent, the edge closes the connection. The
  client reconnects with a new upgrade.
- A minted user never outlives the credential it came from: the edge asks for
  no more validity than the bearer has left.
- Server-initiated messages to a client that is waiting (MCP server-sent events,
  ACP client-direction requests) travel on the subscriptions of the held
  connection. When renewal is due and no request has arrived to carry a
  credential, the stream stays on the current user until it expires and is then
  ended, like the WebSocket.

## Alternatives Considered

### Require AAuth only on the MCP edge

This is the strongest proof and the position of
[ADR#0050](./0050-signed-first-caller-authentication.md). Rejected because stock
MCP clients send an OAuth bearer and do not sign requests. An edge that accepts
only a signature admits none of them. The bearer tier is kept as the labeled
compatibility tier and can be turned off per deployment.

### Require DPoP or another sender-constrained token on the MCP edge

Rejected as the requirement. The core authorization page of the pinned
revision does not mention DPoP or sender-constrained tokens, and this record
does not rely on any client supporting them. If a later revision or an MCP
extension makes a sender-constrained token a client behavior, it joins the edge
as another proof option next to AAuth, bound to the bearer's subject in the same
way.

### Accept the bearer and keep one shared connection per server process

The edge verifies, publishes everything on its own connection, and the principal
goes in the subject or a header. Rejected. The edge's connection would hold a
wildcard grant over every principal position, so a bug or compromise in the edge
would let it speak as anyone. A minted user confines each connection to one
principal, and the cost is one connection per session, which the A2A bridge
already pays per request.

### Carry the principal in a header or the JSON-RPC body

Rejected for the reason ADR 0082 gives for A2A: a header or a body field says
whatever its publisher wrote. [ADR#0041](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md)
also keeps MCP bodies canonical, and an ACP `prompt` body belongs to the ACP
schema.

### Put the principal after the method

Rejected, as ADR 0082 rejects it for deciders. MCP methods have several tokens,
and ACP `ext.>` is open-ended, so the principal would have no fixed position and
a grant could not confine it with a single wildcard. Before the role or method,
a grant is one pattern.

### Keep the marker-search parsers

Rejected. A search for `.server.` or `.agent.` finds the first occurrence, and an
id that is valid under the principal id rules, for example `server`, can equal
a marker. Fixed positions remove the ambiguity; the change to parsing is small
next to the subject change.

### Offer the bearer-only tier on the ACP edge

Rejected for now. The tier exists for tooling that cannot sign, and the ACP
transport shows no such tooling (Decision 3). Adding it without a client that
needs it opens the weaker path for no gain.

### Extend `CallerGrant` with MCP and ACP variants inside the A2A callout crate

Rejected. It leaves protocol-neutral grant code in an A2A-group crate that must
depend on `mcp-nats` and `acp-nats`, against
[ADR#0034](./0034-rust-crate-domain-grouping.md). The code is small enough that
moving it once costs less than carrying the misplacement.

### Enforce session ownership only at the edge

Rejected. The edge sees only the sessions that came through it. A local bridge
or a second edge reaching the same agent with the same session id would not be
checked. The agent holds the session and sees the principal on every command.

### Mint a user valid for the whole session

Rejected. A user JWT that lasts as long as the longest session stretches the
time a revoked caller keeps its grant from minutes to hours, against the aim of
[ADR#0049](./0049-revocation-latency-target.md). Renewal from a fresh
credential keeps the short lifetime.

## Consequences

- An MCP or ACP caller that is not known to the callout cannot publish on the
  bus through these servers. The loopback-only, unauthenticated development
  setup stops working in `Enforce`; it needs the development flag, which is a
  deliberate step.
- Existing MCP and ACP clients that do not present a verified credential, and
  existing deployments that do not run the callout for these surfaces, fail to
  start or fail to connect until configured. Nothing is downgraded silently.
- A responder can authorize on the principal in the subject without trusting a
  header, and can reject a subject that lacks one. Audit attribution for MCP
  and ACP has a principal to record, and the audit record can distinguish
  proven admissions from bearer-only ones.
- The subject shapes change for both protocols. For MCP that is a coordinated
  change to `mcp-nats` subjects, parsers and responders. For ACP it also changes
  JetStream subject lists, consumer filters and stored messages (see Not decided
  here). Direct callers and the stdio bridges change with them.
- Each HTTP session costs a NATS connection and periodic mints. The cost grows
  with concurrent sessions and with the callout's mint rate, and is the price of
  confining the edge.
- The callout's OIDC verifier becomes a hard dependency of two more
  surfaces, with its single-issuer, RSA-only, JWT-only limits. Clients of other
  issuers or with opaque tokens are refused until the verifier is extended.
- Wildcard grants over principal positions are held by MCP server responders
  and ACP agents. They are trusted only to copy the principal, as in ADR 0082;
  narrowing them to the principals that called them is not done here.
- `acp-nats-agent` gains session state keyed by principal. Sessions created
  before the change have no owner and need a rule (see Not decided here).
- Moving the callout and its grant code to `platform/` touches the A2A path and
  its deployment configuration. The first implementation step is the move with
  no behavior change.
- The audience configuration of the callout grows a per-surface dimension. A
  token minted for one surface is refused by another.
- The change is separable by protocol. The MCP edge, the ACP edge and the ACP
  session ownership of Decision 5 can each ship alone, provided the shared
  grant code lands first.

## Not decided here

- **Tenant read path.** Which tenant account a minted MCP or ACP user lives in
  and how a read path (history, replay) is scoped to a tenant is left to the
  tenancy records.
- **ACP stream migration mechanics.** How stored messages under the old
  subjects are moved, replayed or retired, and in what order the streams and
  consumers cut over.
- **Sessions that predate ownership.** Whether they are adopted by the first
  principal to claim them, retired, or refused.
- **Session sharing.** An explicit grant of a session to another principal.
- **MCP task and subscription ownership.** Whether `tasks.cancel` and
  subscription requests are bound to the principal that created the task, in
  the way Decision 5 binds ACP sessions.
- **Scopes.** The scope vocabulary of MCP access tokens and the policy that maps
  a scope to a method.
- **Other token forms.** Multiple issuers, opaque-token introspection and
  non-RSA signatures in the callout's verifier.
- **Mint rate limiting.** Limits on mint requests per caller and per edge.
- **Client credentials at the stdio bridges.** The bridges remain direct
  callers; how they obtain their NATS credentials stays with
  [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md).

## References

- [ADR#0081: Every NATS Connection Uses TLS Unless Explicitly Disabled, and Clients Enter Operator-Mode Accounts Only Through Platform Credentials or the Auth Callout](./0081-nats-tls-by-default-and-operator-mode-admission.md)
- [ADR#0082: Inside the Bus the Principal Is Derived from the Subject, and Proof of Possession Is Enforced Only at the External Edge](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
- [ADR#0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
- [ADR#0034: Rust Crate Domain Grouping](./0034-rust-crate-domain-grouping.md)
- [ADR#0041: Canonical MCP JSON-RPC Bodies over NATS](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)
- [ADR#0094: The Platform Authorizes Third-Party Connections as an OAuth Client Through a Single-Use, Write-Ahead Attempt, and Fetches Untrusted Discovery Data Outside Custody (Draft)](./0094-oauth-authorization-protocol.md)
- [policy enforcement posture proposal](https://github.com/TrogonStack/trogonai/pull/745): Policy enforcement posture (in review)
- MCP Authorization specification, revision 2026-07-28; RFC 9728 (OAuth Protected Resource Metadata); RFC 8707 (Resource Indicators); RFC 9421 (HTTP Message Signatures); RFC 9449 (OAuth DPoP)
