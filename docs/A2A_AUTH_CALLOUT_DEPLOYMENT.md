# A2A auth callout — deployment

Operator runbook for the `a2a-auth-callout` service. Design context: [A2A auth callout sketch](./A2A_AUTH_CALLOUT_SKETCH.md). Build tracker: [A2A auth callout TODO](../A2A_AUTH_CALLOUT_TODO.md).

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
| `AUTH_CALLOUT_SIGNING_SECRET` | HS256 key for User JWT mint and denial JWT sign (dev fallback; replace with custody in #5) |
| `AUTH_CALLOUT_ISSUER` | `iss` on denial authorization-response JWTs (callout identity) |
| `AUTH_CALLOUT_ALLOWED_ACCOUNTS` | Comma-separated tenant account names |
| `AUTH_CALLOUT_OIDC_ISSUER` / `AUTH_CALLOUT_OIDC_AUDIENCES` | OIDC verifier |
| `AUTH_CALLOUT_MTLS_TRUST_ANCHORS` | PEM bundle path for mTLS verifier |

## NATS server pin

Target NATS server minor version for auth-callout wire compatibility is recorded when work unit #6 (wire format) lands. Denial encoding follows [NATS authorization callout](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout) authorization-response claims (`nats.type` = `authorization_response`, `nats.version` = 2).
