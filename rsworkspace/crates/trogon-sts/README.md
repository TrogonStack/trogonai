# trogon-sts

NATS queue-group consumer on `mcp.sts.exchange` that performs RFC 8693–style token exchange: a `subject_token` (bootstrap or upstream mesh JWT) plus `actor_token` (SVID attestation) are validated against the agent registry and trust bundle, then exchanged for a short-lived mesh JWT with narrowed `aud`, narrowed `scope`, and an appended `act_chain` entry.

## Dev mode

1. Start NATS (local `nats-server -js` is enough for KV + request/reply).
2. Generate an RS256 signing PEM for mesh tokens (`MCP_STS_SIGNING_KEY_PATH`).
3. Place a SPIFFE trust-bundle PEM at `MCP_STS_TRUST_BUNDLE_PATH` (v1 accepts `spiffe://…` actor tokens when the bundle file is present).
4. Publish bootstrap issuer JWKS to an HTTPS URL or `kv://mcp-jwks/bootstrap/current`.
5. Seed the agent registry (in-memory stub or real `mcp.registry.agent.lookup` consumer).

Run:

```bash
cargo run -p trogon-sts -- \
  --nats-url nats://127.0.0.1:4222 \
  --signing-key-pem ./dev/mesh-signing.pem \
  --bootstrap-issuer-jwks-url http://127.0.0.1:8080/jwks.json \
  --trust-bundle-path ./dev/spiffe-bundle.pem
```

Exchange (request/reply on `mcp.sts.exchange`):

```json
{
  "subject_token": "<JWT>",
  "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
  "actor_token": "spiffe://acme.local/ns/prod/sa/oncall-agent",
  "audience": "urn:trogon:mcp:backend:acme:github",
  "scope": "tool:github::create_issue",
  "purpose": "oncall-incident-triage",
  "requested_token_type": "urn:ietf:params:oauth:token-type:jwt"
}
```

Success replies include `access_token`, `issued_token_type`, `token_type: Bearer`, and `expires_in` (default **120 s**, overridable per agent via registry `mesh_token_ttl_s`).

## Real registry

Point `--registry-subject` at the live registry service (default `mcp.registry.agent.lookup`). STS fail-closes when lookup returns `not_found`, `revoked`, or transport errors. Registry records are cached in-memory for up to **60 s** per `agent_id`.

## Production requirements

| Concern | v1 | Production target |
|--------|----|-------------------|
| Mesh signing | File PEM (`--signing-key-pem`) | KMS or Vault Transit signer (stubs in `signer.rs`) |
| Workload attestation | File trust bundle + `spiffe://` / `sha256:` actor tokens | SPIRE/SVID-JWT verification |
| Rate limits | In-memory per `wkl` (100/10s) and `agent_id` (500/10s) per instance | Distributed limiter (Redis/NATS) |
| JWKS | HTTP + optional `mcp-jwks` KV watch | JWKS publisher sidecar per ADR 0006 |
| Audit | `mcp.audit.sts.{outcome}` JetStream publish | Same; feed `trogon-decider` aggregates |

Every exchange emits audit events (success, deny, rate-limit, internal error) with request fields (no JWT signatures), minted claim summary, `wkl`, `agent_id`, decision reason, and latency.

## Related docs

- ADR 0003 — bootstrap vs mesh tokens
- ADR 0004 — NATS STS form factor
- ADR 0005 — TTL, `aud`, rate limits
- ADR 0006 — signing-key custody and JWKS
- `docs/identity/registry.md` — registry lookup contract
