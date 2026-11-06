# a2a-gateway Locally

`a2a-gateway` subscribes to the A2A ingress wildcard `a2a.v1.gateway.*.*.*.>`
in the `PLATFORM` account, derives the caller from the subject (ADR 0082),
runs the enabled policy tiers, and forwards a request that passes to its agent.
A request that fails a tier is refused, and a forwarded `A2a-Caller-Jwt` that
does not verify or names another caller is refused before any tier runs.

The image builds `-p a2a-gateway --bin a2a-gateway` with default features, so
there is no SpiceDB tier. The Dockerfile sits here, next to the sibling
`a2a-auth-callout` one, because both are dev-stack images built by compose. The
published images live under `devops/docker/images/`.

## Quick start

From `devops/docker/compose`:

```bash
export TROGON_GATEWAY_LOCAL_GITHUB_WEBHOOK_SECRET=$(openssl rand -hex 32) # or put it in .env
docker compose up --build -d a2a-gateway
docker compose logs a2a-gateway
```

The log line `gateway subscribed on ingress wildcard` means it connected and
subscribed.

## Environment

NATS follows [ADR 0081](../../../../../docs/adr/0081-nats-tls-by-default-and-operator-mode-admission.md):
TLS with a client certificate and an operator-mode creds file. The compose
service sets the shared `NATS_URL`, `NATS_CREDS`, `NATS_CA`, `NATS_CERT` and
`NATS_KEY` and mounts only `clients/a2a-gateway` of the `dev_secrets` volume at
`/run/secrets/nats`, which `nats-bootstrap` and `pki-bootstrap` fill.

| Variable | Source |
|---|---|
| `A2A_GATEWAY_CALLER_JWT_ISSUER_KEYS` | `gateway.env`, written by `nats-bootstrap`. The public key of the seed the callout signs tenant `DEV` users with, which is how the gateway checks a forwarded caller JWT. |
| `A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED` | `true` in `compose.yml` |
| `A2A_GATEWAY_TIER1_BUNDLE_DIR` | `/etc/a2a-gateway/policy`, this directory's `policy/` mounted read-only |
| `A2A_GATEWAY_JWT_AUDIENCE` | `gateway.env`, written by `nats-bootstrap`. The `DEV` account public key, which the callout puts in the `aud` of every caller JWT it mints for that tenant. |
| `A2A_PREFIX`, `A2A_GATEWAY_QUEUE_GROUP` | Optional, unset by default |
| `RUST_LOG` | Defaults to `info` |

The gateway holds only public keys, so it can check a caller JWT but never
mint one.

## Changing the policy bundle

Edit or add `*.tier1.toml` files under `services/a2a-gateway/policy/`. Each
file holds `[[rule]]` tables with an `id`, a `priority`, an `effect` of `allow`
or `deny`, and one or more `[[rule.matches]]` of a `kind` (`agent_method`,
`agent_id`, `caller_subject`, `nats_subject_pattern` or `time_of_day`) and a
`pattern`. All matches on a rule must hit, the highest priority rule that hits
decides, and a request no rule matches is allowed. See `policy/example.tier1.toml`.

The bundle loads once at boot, so apply a change with:

```bash
docker compose restart a2a-gateway
```

The enabled layer fails the boot when `A2A_GATEWAY_TIER1_BUNDLE_DIR` does not
exist, and a malformed file fails it with the file named.
