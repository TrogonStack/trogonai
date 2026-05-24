# A2A auth callout — deployment

Operator runbook for the `a2a-auth-callout` service. Design context: [A2A auth callout sketch](./A2A_AUTH_CALLOUT_SKETCH.md). Build tracker: [A2A auth callout TODO](../A2A_AUTH_CALLOUT_TODO.md).

## Signing key custody

Select custody with `AUTH_CALLOUT_SIGNING_KEY_SOURCE`:

| Value | Use | Notes |
|-------|-----|--------|
| `env` | Local dev only | Default. Reads `AUTH_CALLOUT_SIGNING_SECRET` (+ optional `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS`). Logs a one-time warning that env secrets are not for production. |
| `file` | Production / VMs | Requires `AUTH_CALLOUT_SIGNING_KEY_PATH` (current key material). Optional `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH` for overlap during rotation. Files are raw PEM or shared-secret bytes (no JSON envelope). |
| `vault` | Planned | Not wired yet; process exits if selected. Future work will load keys from an operator vault behind a feature flag. |

Minted User JWTs carry a `kid` claim (and matching JWS header `kid`) set to the key version string (`current` / `previous` for file and env sources). Verifiers should select the decoding key from the source's `accepted()` list using `kid`, falling back to trial verification only when `kid` is absent.

## Rotation procedure

1. Generate new signing material. Deploy callout with **new** key at `current` and **old** key at `previous` (`AUTH_CALLOUT_SIGNING_KEY_PATH` + `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH`, or env equivalents).
2. Wait at least one full User JWT TTL (`AUTH_CALLOUT_USER_JWT_TTL_SECS`, default 300s) so outstanding tokens expire.
3. Redeploy with only the new key at `current`; remove `previous` paths / `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS`.

During overlap, mint always uses `current`; verification accepts both until step 3.

## Denial semantics

When `AuthDispatcher::dispatch` rejects a connect attempt, the subscriber replies with a signed **authorization response** JWT (NATS auth-callout extension). The JWT omits `nats.jwt` and sets `nats.error` to an opaque category string. The server logs that string; clients must not rely on it for security decisions.

| `nats.error` category | Meaning for operators |
|----------------------|------------------------|
| `invalid_credentials` | Presented credential failed verification (OIDC, mTLS, or API key), or policy rejected the principal. Check IdP health, cert trust, and key registry. |
| `unknown_account` | Requested tenant account is not in the callout allowlist (`AUTH_CALLOUT_ALLOWED_ACCOUNTS`). |
| `invalid_request` | Request shape is incomplete (missing account, missing credential material, or scheme/credential mismatch). |
| `verifier_unavailable` | Required verifier is not configured on the callout process (e.g. OIDC issuer set but JWKS discovery failed). |
| `internal_error` | Callout-internal failure (serialization, reply publish, JWT mint). Inspect callout logs with full `error` field. |
| `service_unavailable` | Callout could not reach NATS or subscribe to `$SYS.REQ.USER.AUTH`. |

Structured audit logs on denial include `reason_category`, `request_jti` (when the server request was JWT-encoded), `caller_id_hint` (untrusted `user_nkey` or claimed account), and the full internal error for operators. Wire categories never include OIDC, x509, or verifier exception text.

Denial JWTs require `server_id` (server NKey from the request `iss`, echoed as response `aud`) and `user_nkey` (response `sub`). Until the wire-format task replaces the illustrative JSON request envelope, operators testing denials must populate those fields on the request payload.

## Environment (summary)

| Variable | Role |
|----------|------|
| `NATS_URL` | Callout process NATS connection |
| `AUTH_CALLOUT_SIGNING_KEY_SOURCE` | `env` \| `file` \| `vault` — selects signing key custody |
| `AUTH_CALLOUT_SIGNING_SECRET` / `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS` | HS256 key material when source is `env` |
| `AUTH_CALLOUT_SIGNING_KEY_PATH` / `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH` | Key file paths when source is `file` |
| `AUTH_CALLOUT_ISSUER` | `iss` on minted User JWTs and denial authorization-response JWTs (callout identity) |
| `AUTH_CALLOUT_ALLOWED_ACCOUNTS` | Comma-separated tenant account names |
| `AUTH_CALLOUT_OIDC_ISSUER` / `AUTH_CALLOUT_OIDC_AUDIENCES` | OIDC verifier |
| `AUTH_CALLOUT_MTLS_TRUST_ANCHORS` | PEM bundle path for mTLS verifier |

Full env reference: [`A2A_RUNTIME_ENV.md`](./A2A_RUNTIME_ENV.md).

## NATS server pin

Target NATS server minor version for auth-callout wire compatibility is recorded when work unit #6 (wire format) lands. Denial encoding follows [NATS authorization callout](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout) authorization-response claims (`nats.type` = `authorization_response`, `nats.version` = 2).

## Related

- Runtime env reference: [`A2A_RUNTIME_ENV.md`](./A2A_RUNTIME_ENV.md)
- Build tracker: [`A2A_AUTH_CALLOUT_TODO.md`](../A2A_AUTH_CALLOUT_TODO.md)
