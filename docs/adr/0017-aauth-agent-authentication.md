---
number: "0017"
slug: aauth-agent-authentication
status: accepted
date: 2026-07-07
---

# ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding

## Context

The [A2A](../glossary/a2a) gateway is the ingress boundary for agent-to-agent traffic on the mesh
([ADR#0003](./0003-ai-protocol-transport-taxonomy.md)). Before any authorization
policy runs (declarative, SpiceDB, CEL, or redaction), the gateway has to answer
a prior question: which agent is making this call, and can it prove it?

OAuth 2.0 and OpenID Connect assume a human-registered client: a `client_id`
issued by each authorization server, meaningless outside that server's context,
provisioned by a developer visiting a portal ahead of time. Agents do not fit
that shape. They discover resources and other agents at runtime, run long tasks
across trust domains, and need an identity that is theirs -- not borrowed from a
human's pre-registered app -- before the first call.

`draft-hardt-oauth-aauth-protocol` (AAuth) starts from a different premise:
every agent has its own cryptographic identity, bound to a signing key,
verifiable without pre-registration or a shared secret. It defines three token
types the gateway cares about:

- `aa-agent+jwt`: issued by an Agent Provider at bootstrap, binding an agent
  identifier to a `cnf.jwk` public key. This is the agent's self-sovereign
  identity token and the key it signs every request with.
- `aa-resource+jwt`: issued by a resource (here, the gateway) as a challenge
  when it denies a request, describing the scope and audience the agent must
  go obtain authorization for.
- `aa-auth+jwt`: issued by a Person Server (three-party mode) or an Access
  Server behind PS federation (four-party mode), asserting a person-linked
  identity and/or authorized scope for the presenting agent.

The draft's [transport](../glossary/transport) binding is HTTP: agents sign requests with HTTP Message
Signatures ([RFC 9421](https://www.rfc-editor.org/rfc/rfc9421)), presenting the
signing key via a `Signature-Key` header (`scheme=jwt`, carrying either the
agent token or, once authorized, the auth token) and covering `@method`,
`@authority`, `@path`, and `signature-key` at minimum, with `content-digest`
required by servers that need body integrity. Verification failures produce a
`401` carrying an `AAuth-Requirement` challenge naming what the agent still
needs.

Our transport is [NATS](../glossary/nats), not HTTP
([ADR#0003](./0003-ai-protocol-transport-taxonomy.md)). NATS has no method, no
authority, no path, and no `Signature-Key` header -- subjects and headers are
the only structural surface available, and NATS header values already exclude
CR/LF but otherwise place little constraint on the signing input. Adopting
AAuth verbatim gives us the token model, the claim sets, and the verification
rules; it does not tell us how to carry a signed request over NATS. That
binding has to be defined the way [ADR#0011](./0011-jsonrpc-over-nats-binding.md)
and [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) defined [JSON-RPC](../glossary/json-rpc)
and protobuf bindings for the same backbone, rather than invented ad hoc at each
call site.

## Decision

### 1. Adopt the draft's token model and verification rules unchanged

`aa-agent+jwt`, `aa-resource+jwt`, and `aa-auth+jwt` keep their claim sets,
`typ` values, and verification steps exactly as the draft defines them:
`iss`/`dwk`/`sub`/`jti`/`cnf.jwk` for the agent token; `iss`/`aud`/`agent`/
`agent_jkt`/`scope` for the resource challenge; `iss`/`sub`/`aud`/`agent`/
`agent_jkt`/`scope`/`principal` for the auth token. JWKS discovery, `cnf.jwk`
algorithm support (ES256, ES384, EdDSA), and the agent-token-then-auth-token
identity chain are unmodified. `trogon-identity-types::aauth` is the wire-type
crate for these claim sets, and `trogon-aauth-verify` is the only place JWT
signature and PoP verification logic lives.

### 2. Define a Trogon NATS PoP binding that mirrors RFC 9421 component coverage

Since NATS has no HTTP request line, the binding replaces the RFC 9421
derived components with the closest NATS equivalents and carries the
signature envelope in Trogon-defined headers instead of `Signature-Input` /
`Signature` / `Signature-Key`:

| RFC 9421 (HTTP) | Trogon NATS binding | Header |
| --- | --- | --- |
| `@method`, `@authority`, `@path` | `@subject` | (part of canonical base, not a header) |
| n/a (no HTTP reply concept) | `@reply` | (part of canonical base, not a header) |
| `content-digest` | `content-digest` | `Content-Digest` |
| `Signature-Key` (agent/auth token) | `token` | `AAuth-Token` |
| `Signature-Input` | `sig_input` | `AAuth-Sig-Input` |
| `Signature` | `sig` | `AAuth-Sig` |
| `created` parameter | `created` | `AAuth-Sig-Created` |
| n/a (HTTP has no nonce mechanism) | `nonce` | `AAuth-Sig-Nonce` |

The canonical signature base is built by
`trogon_identity_types::aauth::NatsSignatureEnvelope::canonical_base`, over
`@subject`, `@reply` (empty string when there is no reply subject),
`content-digest`, the `aa-agent+jwt` in `AAuth-Token`, `AAuth-Sig-Created`, and
`AAuth-Sig-Nonce`, closed out with `@signature-params` binding the covered
component list, `created`, and the signing key's `keyid` (the JWK thumbprint).
This is the NATS analogue of RFC 9421's covered-component list: `@subject`
plays the role `@method`/`@authority`/`@path` play together (there is exactly
one routing fact on a NATS message, the subject), `@reply` covers the reply
target the draft has no equivalent for because HTTP responses don't need one,
and `content-digest`, the token, and `created` are carried the same way the
draft carries them. The nonce is Trogon-defined: RFC 9421 relies on `created`
plus an optional verifier-side replay cache keyed by
`(signing-key-thumbprint, created, @method, @authority, @path)`; NATS request-
reply has no per-request URL component to key that cache on, so the binding
requires an explicit nonce and makes replay rejection structural
(`trogon-aauth-verify::nats_pop::NatsPopVerifier`) rather than a MAY.

`Content-Digest` is mandatory on every signed NATS message, not opt-in the way
the draft leaves it for HTTP (`content-digest` is a MAY for servers that want
body integrity). The verifier recomputes the digest from the message payload
and requires it to match the supplied header rather than synthesize a digest
when the header is absent -- an absent-and-synthesized digest would let an
attacker drop the header and still pass, since the signature would then cover
a digest the verifier itself computed rather than a client claim it checked.

Any of the six security-sensitive headers
(`AAuth-Token`, `AAuth-Sig-Input`, `AAuth-Sig`, `AAuth-Sig-Created`,
`AAuth-Sig-Nonce`, `Content-Digest`) appearing more than once on an inbound
message is a hard failure, not a first-value-wins pick, closing a header-
smuggling path where the signature checks one value while a downstream
consumer reads another.

### 3. Carry the auth token in a Trogon-defined `AAuth-Auth-Token` header

The draft presents `aa-auth+jwt` the same way it presents `aa-agent+jwt`: via
`Signature-Key`, once the agent has exchanged its resource challenge for an
auth token at its Person Server. NATS has no `Signature-Key` header to
overload, and the PoP signature still needs the agent's own `cnf.jwk` to
verify against regardless of which principal is asserted -- so the two tokens
cannot share one slot the way HTTP's single header does. The binding adds
`AAuth-Auth-Token` as a Trogon-defined header carried alongside `AAuth-Token`:
`AAuth-Token` always holds the `aa-agent+jwt` the PoP signature verifies
against, and `AAuth-Auth-Token`, when present, holds the `aa-auth+jwt` the
gateway additionally checks and binds to that same agent (`agent` and
`agent_jkt` claims must match the PoP-verified agent's `sub` and JWK
thumbprint; a mismatch is a distinct deny reason,
`AAuthDenyReason::AuthAgentMismatch`, so agent impersonation via a foreign auth
token is auditable separately from a plain verification failure).

### 4. Reserve JSON-RPC error code `-32118` for AAuth denials on gateway ingress

When the gateway denies a request under AAuth (PoP failure, auth-token
verification failure, or agent/auth-token binding mismatch), the reply is a
JSON-RPC error with code `-32118`
(`a2a_gateway::aauth::AAUTH_REQUIRED_CODE`) per the JSON-RPC-over-NATS binding
in [ADR#0011](./0011-jsonrpc-over-nats-binding.md) -- the code rides in the
`Jsonrpc-Error-Code` header, discriminating the reply as an error the same way
every other JSON-RPC error on the mesh does. Alongside it, the deny reply
carries an `AAuth-Requirement` header on the reply message, in the same role
the draft's `401` response plays with its own `AAuth-Requirement` header: it
names what the agent still needs (`requirement=auth-token;
resource-token="<aa-resource+jwt>"`), minted and bound to the PoP-verified
agent so the agent can present it to its Person Server. The header value
format (`requirement=...; key="value"; ...`) is unchanged from the draft;
`trogon_identity_types::aauth::Requirement` parses and renders it identically
on both the HTTP and NATS paths.

### 5. AAuth is authentication and runs before tier-1/2/3 authorization

The gateway's policy stack layers Tier 1 declarative and SpiceDB relational
checks, Tier 2 CEL predicates, and Tier 3 Wasm redaction on top of a resolved
caller identity. AAuth sits before all of it: `AAuthIngress::resolve_nats`
runs ahead of `runtime::dispatch_gateway_ingress`'s tier-1 stage and produces
an `AAuthResolution` (or an `AAuthDeny`) that the dispatch path treats as the
identity input to policy, not as a policy decision itself. When an
`aa-auth+jwt` principal verifies, it supersedes the JWT-header caller identity
that `jwt_caller_identity` would otherwise resolve -- the person-asserted
identity from the Person Server is authoritative over whatever the caller's
own NATS User JWT claims, because it is the one the resource (the gateway)
itself challenged for and verified end to end. Tier 1 through Tier 3 then run
against whichever identity AAuth resolved.

### 6. The gateway layer is env-configured off / shadow / enforce and fails loudly

`a2a_gateway::runtime::aauth_env::gateway_aauth_from_env` resolves the mode
from `A2A_GATEWAY_AAUTH_MODE` (`off` | `shadow` | `enforce`, default `off`).
`off` skips AAuth entirely -- dispatch never sees the layer. `shadow` runs
verification, logs a denial (`event = "aauth.shadow_deny"`) with the typed
deny reason, and lets the request through as anonymous, so the mode can be
measured against production traffic before it blocks anything. `enforce`
denies with `-32118` and the `AAuth-Requirement` challenge.

Once `shadow` or `enforce` is selected, every required var
(`A2A_GATEWAY_AAUTH_RESOURCE_ISS`, `A2A_GATEWAY_AAUTH_PERSON_SERVER_AUD`,
`A2A_GATEWAY_AAUTH_CHALLENGE_KID`, `A2A_GATEWAY_AAUTH_CHALLENGE_KEY_PATH`)
must resolve, and exactly one JWKS source must be configured -- either a
static file (`A2A_GATEWAY_AAUTH_JWKS_PATH`) or live well-known discovery
(`A2A_GATEWAY_AAUTH_JWKS_DISCOVERY=true`, cached with
`A2A_GATEWAY_AAUTH_JWKS_TTL_SECS`) -- or the gateway fails to
start with a typed `AAuthEnvError` naming the offending variable. There is no
silent fallback to `Off` on misconfiguration -- matching the failure posture
the tier-1 SpiceDB layer already uses. Optional tuning
(`A2A_GATEWAY_AAUTH_LEEWAY_SECS`, `A2A_GATEWAY_AAUTH_CHALLENGE_TTL_SECS`,
`A2A_GATEWAY_AAUTH_MAX_SKEW_SECS`) defaults to 60s / 300s / 60s respectively
when unset, but an unparseable value is still a startup error rather than a
silently-ignored default.

## Consequences

- Agents authenticate to the gateway with a self-sovereign, key-bound identity
  instead of a pre-registered OAuth client, matching how the draft expects
  agent-to-resource calls to work.
- The NATS PoP binding is a shared, header-driven mechanism
  (`trogon-aauth-verify::nats_pop`) rather than a per-call-site scheme, the
  same posture [ADR#0011](./0011-jsonrpc-over-nats-binding.md) and
  [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) take for their
  bindings. `@subject` and
  `@reply` stand in for the HTTP derived components the draft covers; there is
  no request line to borrow from.
- `Content-Digest` is required, not optional, on every AAuth-signed NATS
  message, and duplicate security headers are rejected outright -- both are
  stricter than the draft's HTTP profile because NATS gives the binding fewer
  structural guarantees (no method/path/host) to lean on.
- `AAuth-Auth-Token` is a Trogon invention with no draft counterpart: it exists
  because NATS has no `Signature-Key` slot that can carry two token kinds at
  once, and the agent token must stay presentable for PoP verification
  alongside whatever auth token the Person Server issued.
- `-32118` is now reserved on the JSON-RPC-over-NATS error surface for AAuth
  denials specifically; no other gateway error path may reuse it.
- Replay protection today is `InMemoryReplayStore`, process-local. A
  multi-node gateway deployment can have the same nonce accepted once per
  node until a shared store (NATS [JetStream](../glossary/jetstream) KV, per the doc comment in
  `trogon-aauth-person`) is wired in; single-node deployments are fully
  protected, multi-node deployments are not yet.
- JWKS resolution is env-selected between a static file (`StaticJwks`) and
  live `.well-known/{dwk}` discovery (`HttpJwksResolver`, HTTPS-only, size-
  and timeout-capped, wrapped in `CachedJwksResolver`). Static deployments
  still require a restart to rotate issuer keys; discovery deployments get
  the draft's cache-and-refresh behavior.
- The protocol roles beyond the gateway ship as sibling crates:
  `trogon-aauth-person` (three-party Person Server: token endpoint with
  interaction / clarification / approval flows, permission, audit and
  interaction endpoints, missions, third-party login), `trogon-aauth-as`
  (four-party Access Server federation with claims-required and `act`
  nesting), `trogon-aauth-sdk` (agent-side signer plus the challenge-to-
  auth-token exchange, call chaining, sub-agents), and
  `trogon-jwks-publisher` (Agent Provider issuance and well-known JWKS
  publishing). These are libraries with HTTP bindings and in-memory stores;
  deploying them as durable, multi-node services (persistent pending-request
  and mission stores, operational wiring) is follow-up work, not part of
  this ADR.
- The draft is a moving IETF Internet-Draft, not a stable RFC. This ADR pins
  implementation to
  [`draft-hardt-oauth-aauth-protocol`](https://github.com/dickhardt/AAuth/blob/main/draft-hardt-oauth-aauth-protocol.md)
  at commit `90089f80eaccccbd22e32e06946e2aa08f7d67fe` on `main`. A future
  revision that changes claim names, the `AAuth-Requirement` header grammar,
  or the covered-component set requires a follow-up [ADR](../glossary/adr) to re-pin and
  reconcile `trogon-identity-types::aauth`, not a silent code update.

## References

- [draft-hardt-oauth-aauth-protocol](https://raw.githubusercontent.com/dickhardt/AAuth/refs/heads/main/draft-hardt-oauth-aauth-protocol.md)
  (pinned at commit `90089f80eaccccbd22e32e06946e2aa08f7d67fe`)
- [RFC 9421: HTTP Message Signatures](https://www.rfc-editor.org/rfc/rfc9421)
- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR#0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
- [ADR#0016: Protocol Buffers RPC over NATS micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
