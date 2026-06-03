# AAuth End-to-End Handshake (HTTP + NATS)

Status: implemented on `yordis/agentgateway`. Companion to
[aauth-design.md](./aauth-design.md) — that document records the design
decisions, this one walks the runtime flow that the implementation actually
executes and points at the crates / tests that prove each step.

This is the Posta supply-chain / market-analysis demo, mapped onto Trogon's
NATS-first wiring. Every step has an HTTP variant and a NATS variant; both
share the same Person Server (PS) and the same agent identity token.

## Cast

- **Agent Provider (AP):** stand-in here for the Person Server itself in 3-party
  mode (`PersonCore` in `trogon-aauth-person`). In 4-party mode the AP is a
  separate issuer.
- **Person Server (PS):** issues `aa-auth+jwt` after evaluating consent
  (`trogon-aauth-person::PersonCore`).
- **Resource:** any service behind a Trogon gateway — `trogon-mcp-gateway`
  enforces AAuth via `aauth.rs::AAuthIngress` for both HTTP and NATS.
- **Agent:** uses `trogon-aauth-sdk` to hold its keypair, bootstrap with the
  PS, sign outbound requests, and recover from challenges.

## Tokens at a glance

| `typ` value         | Issuer | Subject (`sub`)  | Holds                           | Crate                     |
| ------------------- | ------ | ---------------- | ------------------------------- | ------------------------- |
| `aa-agent+jwt`      | PS/AP  | `aauth:<id>@…`   | `cnf.jwk` of the agent          | `trogon-aauth-person`     |
| `aa-resource+jwt`   | Resource | `n/a`          | `agent_jkt`, `scope`, `aud=PS`  | `trogon-mcp-gateway`      |
| `aa-auth+jwt`       | PS     | principal/agent  | `agent_jkt`, `scope`, `aud=Res` | `trogon-aauth-person`     |

All three are signed ES256 (P-256 ECDSA) for v0. JWKS distribution lives in
`trogon-jwks-publisher` (NATS KV + req/rep) and at `/.well-known/jwks.json`
when the issuer also speaks HTTP.

## Step 1 — Day-Zero bootstrap

```
Agent                       PS (PersonCore::bootstrap)
─────                       ──────────────────────────
keypair = AgentKeypair::generate()      // P-256, JKT = RFC 7638 thumbprint
POST /aauth/agent {cnf_jwk, principal}  // or NATS req aauth.{ps_id}.bootstrap
                            ─────────────────────────────►
                            store.put_agent(record)
                            mint aa-agent+jwt
                            ◄───────────────── { agent_jwt, agent_id, expires_in }
```

SDK helpers:

- HTTP: `PersonHttpClient::bootstrap(BootstrapRequest{...})`
- NATS: `PersonNatsClient::bootstrap(...)` on subject `aauth.{ps_id}.bootstrap`

The agent persists `agent_jwt` and its private key (PKCS#8 PEM via
`AgentKeypair::from_pkcs8_pem` on restart). The PS stores the JWK record so it
can later attest to consent.

## Step 2 — Agent calls a resource

The first call carries the agent identity and proves possession of the
`cnf.jwk` — but no `aa-auth+jwt` yet.

### NATS path

`NatsRequestSigner::sign` attaches:

- `AAuth-Token` — the `aa-agent+jwt`.
- `AAuth-Sig-Input`, `AAuth-Sig`, `AAuth-Sig-Created`, `AAuth-Sig-Nonce`.
- `Content-Digest: sha-256=:…:` over the payload.

`NatsPopVerifier::verify` (used by `AAuthIngress::resolve_nats`) reconstructs
the canonical base, validates the JWS signature against `cnf.jwk`, enforces
the replay window, and returns `VerifiedAgent { claims, jkt }`.

### HTTP path

`HttpRequestSigner::sign` produces RFC 9421 `Signature-Input` / `Signature`
plus `Content-Digest`, attached to the request. The gateway runs the verifier
against `@method`, `@path`, `@authority`, `content-digest`, `aauth-token`.

## Step 3 — Resource issues a challenge

If the agent is known but lacks the right `aa-auth+jwt`, the resource (or the
gateway acting on its behalf) mints an `aa-resource+jwt` via
`ChallengeMinter::mint` and returns:

- HTTP: `401` + `AAuth-Requirement: requirement=auth-token; resource-token="…"`
- NATS: reply headers `Nats-Service-Error-Code: 401` and the same
  `AAuth-Requirement` value.

`AAuthDeny::to_requirement_header()` in `trogon-mcp-gateway::aauth` is the
single place that constructs that header.

## Step 4 — Agent exchanges with the PS

The SDK parses the challenge with `parse_challenge_headers`. The returned
`ChallengeOutcome::NeedsExchange { resource_jwt }` is fed straight into
`PersonHttpClient::exchange` (or its NATS twin) on `/aauth/token`
(`aauth.{ps_id}.token`).

`PersonCore::exchange`:

1. Verifies the `aa-agent+jwt` (signature, typ, freshness).
2. Verifies the `aa-resource+jwt` against the resource's JWKS (same resolver).
3. Asserts `aud == PS iss` and `agent_jkt == agent.jkt`.
4. Calls `ConsentPolicy::decide(...)`.
5. Persists the consent record.
6. Mints an `aa-auth+jwt` with `aud = resource_iss`, `agent_jkt`, `scope`,
   `principal`, `consent_id`.

If the policy returns `ConsentDecision::Interaction`, the response is a `202`
with `{requirement: "interaction", url, code}` — the SDK surfaces this as
`SdkError::Interaction { url, code }`.

## Step 5 — Agent retries with the auth token

The agent re-sends the original request, this time attaching the
`aa-auth+jwt` (alongside the still-valid `aa-agent+jwt` for PoP). The gateway
runs `TokenVerifier::verify_auth(auth_jwt, resource_iss)` which returns
`VerifiedAuth { claims }`. `AAuthResolution::attach_auth` exposes the
principal up to the gateway's policy layer through the existing
`GatewayIdentity` shape.

## Mapping to crates and tests

| Step | Code                                                                   | Test                                                                                   |
| ---- | ---------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| 1    | `trogon-aauth-sdk::PersonHttpClient`, `PersonNatsClient`               | `crates/trogon-aauth-person/tests/end_to_end.rs::bootstrap_then_exchange_succeeds`     |
| 2    | `trogon-aauth-sdk::sign`, `trogon-aauth-verify::nats_pop`              | `crates/trogon-aauth-verify/tests/nats_pop_roundtrip.rs`                               |
| 3    | `trogon-mcp-gateway::aauth::AAuthIngress::resolve_nats`                | `crates/trogon-mcp-gateway` lib tests (`aauth_*`)                                      |
| 4    | `trogon-aauth-person::PersonCore::exchange`                            | `crates/trogon-aauth-person/tests/end_to_end.rs`                                       |
| 5    | `trogon-aauth-verify::TokenVerifier::verify_auth`                      | `crates/trogon-aauth-sdk/tests/gateway_demo.rs::agent_recovers_from_resource_challenge` |

The full handshake — including the resource-side challenge minting, the
challenge parsing, the exchange, and the verifier accepting the resulting
`aa-auth+jwt` — runs end-to-end in
`crates/trogon-aauth-sdk/tests/gateway_demo.rs`.

## Deferred from v0

- 2-party (agent presents `aa-auth+jwt` minted by its own AP).
- 4-party federation between AP and PS.
- Mission tokens (`mission` claim already shipped in the wire types, but
  `PersonCore` doesn't broker mission approvers yet).
- Human interaction relay (the wire shape is supported; the redirect UI is
  not).
- NATS KV–backed `ReplayStore` (`InMemoryReplayStore` ships for single-process
  deployments; the trait + persistence layout for KV is documented in
  `aauth-design.md::D6`).

## Pointers

- Spec: `draft-hardt-aauth-protocol-02`, `draft-hardt-aauth-bootstrap-01`.
- Posta walkthrough: https://blog.christianposta.com/aauth-full-demo/.
- Design decisions (locked): [aauth-design.md](./aauth-design.md).
- Plan / status: [AAUTH_PLAN.md](../../AAUTH_PLAN.md).
