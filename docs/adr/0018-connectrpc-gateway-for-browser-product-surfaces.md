---
number: "0018"
slug: connectrpc-gateway-for-browser-product-surfaces
status: accepted
date: 2026-07-08
---

# ADR#0018: ConnectRPC Gateway for Browser Product Surfaces

## Context

The platform is getting its first product-facing web application, the operator
console ([ADR#0019](./0019-console-webapp-stack.md)). A browser has to call
first-party platform services, and every existing first-party RPC path lives on
the NATS backbone: protobuf services bind to NATS micro
([ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md)), the JSON-RPC
family binds to NATS subjects
([ADR#0011](./0011-jsonrpc-over-nats-binding.md)), and every signed request on
the mesh carries the AAuth NATS PoP envelope
([ADR#0017](./0017-aauth-agent-authentication.md)).

NATS itself does not keep a browser off the backbone. `nats-server` ships a
WebSocket listener, `nats.ws` runs in browsers, and NATS has real
connection-level authentication: passwords, tokens, NKey challenge-response,
decentralized user JWTs, and auth callout, which can mint an ephemeral,
subject-scoped NATS user from an external credential such as a web session. A
browser connection to the backbone is technically achievable.

Connection authentication is not the boundary that matters here. First-party
requests on the mesh carry the AAuth PoP envelope: each request is signed with
a `cnf.jwk`-bound private key, with nonce and content-digest bookkeeping. A
browser tab cannot provide that custody, because any key the page's scripts
can use, an injected script can use and exfiltrate. Putting browsers on the
bus would also make the internal subject namespace an internet-facing surface
whose authorization story is NATS subject permissions rather than a reviewed
method allowlist, and it would require a JavaScript reimplementation of the
[ADR#0011](./0011-jsonrpc-over-nats-binding.md)/[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md)/[ADR#0017](./0017-aauth-agent-authentication.md)
bindings that must track the Rust implementations forever.

[ADR#0003](./0003-ai-protocol-transport-taxonomy.md) already selects the API
style for this situation. Its boundary selection order names
"browser-compatible HTTP access" as a trigger for a first-party service API and
prefers ConnectRPC for that surface. What
[ADR#0003](./0003-ai-protocol-transport-taxonomy.md) does not decide is where the
boundary lives, who holds which credentials, and how a ConnectRPC method
reaches a NATS micro endpoint. Without one rule, each product surface would
invent its own bridge, its own token custody, and its own error mapping, the
same per-call-site drift [ADR#0011](./0011-jsonrpc-over-nats-binding.md) and
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) exist to prevent.

## Decision

### 1. Product web surfaces reach the mesh only through a gateway

A browser product surface talks to a first-party gateway service exposing
ConnectRPC over HTTPS. The gateway is a gateway in the
[ADR#0003](./0003-ai-protocol-transport-taxonomy.md) sense: a production edge
component that accepts external traffic and routes it inward,
containing a bridge onto the backbone.

The ConnectRPC surface is generated from the same `.proto` sources that define
the backbone services ([ADR#0009](./0009-protocol-buffers-wire-contracts.md)).
Browser clients are generated with `protobuf-es` and `connect-es` from the
same Buf pipeline that generates the Rust code. There is no hand-written HTTP
client, no parallel OpenAPI document, and no GraphQL layer; the exceptions
[ADR#0003](./0003-ai-protocol-transport-taxonomy.md) allows for OpenAPI remain
exceptions and are not triggered by a first-party browser surface.

Browsers do not connect to NATS directly, even though the WebSocket listener
and auth callout would make a scoped connection possible. Connection-level
NATS auth cannot substitute for the per-request AAuth PoP envelope, and the
envelope is exactly what a page cannot sign safely. Browser NATS access, if a
narrow read-only case ever justifies it, requires its own decision and does
not weaken this default.

### 2. The gateway is the credential boundary

The human operator authenticates to the gateway with an OAuth 2.0
Authorization Code + PKCE flow. The gateway completes the code exchange, holds
the resulting tokens server-side, and issues an HttpOnly, SameSite session
cookie to the browser. The browser never receives an AAuth token, a signing
key, or a NATS credential of any kind.

Browser auth is cookie-based, and token-in-page patterns are prohibited: no
access, refresh, ID, or session token in `localStorage`, `sessionStorage`,
IndexedDB, JavaScript-readable cookies, or long-lived JavaScript memory, and
no `Authorization: Bearer` header minted by page code. The session cookie is
an opaque identifier, not a JWT; session state lives in the gateway. Anything
JavaScript can read, an injected script can exfiltrate; an HttpOnly cookie
limits an XSS to riding the live session, which is the strictly smaller
failure. Cookie-based auth carries CSRF obligations, which the gateway owns:
`SameSite` on the cookie, strict `Origin` checking on every state-changing
request, and CORS locked to the product surface's origin.

On the mesh, the gateway is an agent under
[ADR#0017](./0017-aauth-agent-authentication.md): it holds its own
`aa-agent+jwt` and signing key, signs every backbone request with the Trogon
NATS PoP binding, and, when acting on behalf of an authenticated operator,
presents the operator-linked `aa-auth+jwt` in `AAuth-Auth-Token` alongside its
own agent token. Person-linked authorization therefore rides the same
mechanism every other agent on the mesh uses; the gateway adds no parallel
identity scheme.

Auth layering is explicit. NATS auth callout (`a2a-auth-callout`) is
connection admission: it decides which clients may attach to the backbone and
scopes their subject permissions, and the gateway attaches under that
admission like any other mesh client. The AAuth PoP envelope is request
authentication, and the policy tiers are authorization. Connection admission
does not substitute for either layer above it, which is why a browser
admitted through auth callout would still be unable to make signed
first-party calls.

### 3. The bridge is mechanical and holds no business logic

[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) binds protobuf method
names to NATS subjects deterministically, so
the gateway maps traffic without per-method invention:

- A unary Connect RPC becomes one NATS request-reply on the bound subject.
- A server-streaming Connect RPC bridges a NATS subscription into one Connect
  stream, scoped to the operator's session.
- Error mapping is canonical: the NATS micro error channel carries the
  gRPC-idiom status semantics
  [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) defines, and Connect
  uses the same
  canonical status codes, so the gateway translates status and message without
  inventing an error vocabulary.

The gateway performs translation, session handling, authorization screening,
and streaming fan-out. It does not aggregate, decide, or own domain rules.
When a handler needs business logic, that logic belongs in a platform service
behind the backbone, and the gateway exposes that service's method instead.

### 4. Exposure is explicit and default-closed

Being an [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) service does
not make a method browser-reachable. The
gateway exposes an explicit allowlist of services and methods; everything else
on the backbone is unreachable from the browser surface. Adding a method to a
product surface is a reviewed gateway change, not a side effect of deploying a
backbone service.

### 5. One gateway workload per product surface family

A gateway is one operated workload in the
[ADR#0003](./0003-ai-protocol-transport-taxonomy.md) combined-binary sense: one
deployment unit, one telemetry identity, one security boundary. It may also
serve the static assets of its product surface when the assets share
ownership and release cadence, keeping a product surface deployable as one
service. Distinct product surfaces with different audiences, trust levels, or
release cadences get their own gateway workload rather than sharing one
allowlist and session model.

## Consequences

- The repository gains a new workload class, the product surface gateway. The
  console gateway is its first instance.
- Browser code consumes generated Connect clients, so the `.proto` sources
  remain the single wire contract from browser to backbone service.
- Trace context propagates end to end under
  [ADR#0008](./0008-opentelemetry-observability.md): the browser sends
  `traceparent` on every Connect call and the gateway continues that trace
  onto its NATS micro requests.
- Live updates in a browser surface are Connect server streams fed by NATS
  subscriptions in the gateway; product surfaces do not open their own NATS
  connections.
- The gateway's session store and PoP signing make it stateful in the same
  ways the A2A gateway already is; replay-store and multi-node caveats from
  [ADR#0017](./0017-aauth-agent-authentication.md) apply to it equally.
- A future non-browser consumer that needs an explicit API surface (partner
  integration, external tooling) can reuse the same ConnectRPC surface
  without a new decision, because
  [ADR#0003](./0003-ai-protocol-transport-taxonomy.md) already prefers ConnectRPC
  there.

## References

- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
- [ADR#0016: Protocol Buffers RPC over NATS micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ConnectRPC protocol reference](https://connectrpc.com/docs/protocol/)
- [Protobuf-ES](https://github.com/bufbuild/protobuf-es)
