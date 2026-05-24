# A2A auth callout — deployment reference

Operator runbook for the `a2a-auth-callout` service on NATS `$SYS.REQ.USER.AUTH`. Design context: [A2A auth callout sketch](./A2A_AUTH_CALLOUT_SKETCH.md). Build tracker: [A2A auth callout TODO](../A2A_AUTH_CALLOUT_TODO.md).

## Pinned versions

| Component | Version | Notes |
|-----------|---------|--------|
| **nats-server** | **2.14.x** (validated against **2.14.1** server source) | Auth callout extension ships from [v2.10.0](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout.md); pin to the latest 2.14 line for current JetStream + leafnode fixes. |
| **nats-jwt-rs** | **0.1.1** | Authorization request/response JWT encode/decode. |
| **nkeys** | **0.4.5** (`xkeys` feature) | NKey signing and XKey encrypt/decrypt for callout payloads. |

Spec references:

- [Auth callout configuration](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout.md)
- [`nats-server` `server/auth_callout.go`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/auth_callout.go) (request/reply encoding)
- [NKeys / JWT tutorials](https://docs.nats.io/using-nats/developer/tutorials/jwt)

## Wire format (server path)

1. **Request** — `nats-server` publishes to `$SYS.REQ.USER.AUTH`:
   - Payload: server-signed JWT (`ed25519-nkey`) whose `aud` is `nats-authorization-request` and whose `nats` block carries `server_id`, `user_nkey`, `client_info`, `connect_opts`, optional `client_tls`.
   - Optional: payload encrypted with the account **XKey** (server seals with its persistent XKey; callout decrypts using `AUTH_CALLOUT_XKEY_SEED` + `AUTH_CALLOUT_SERVER_XKEY_PUBLIC`). Header `Nats-Server-Xkey` is the server **one-time** public key used only when encrypting the **response** back to the server.
2. **Response** — callout replies on the message inbox:
   - Payload: callout-signed JWT whose `sub` is the request `user_nkey`, `aud` is `server_id.id`, and `nats.jwt` holds the minted user credential (or `nats.error` on denial).
   - Optional: encrypt response with account XKey to the server one-time public key from `nats.server_id.xkey`.

## Bridge path (internal JSON)

The HTTPS bridge does **not** speak the server JWT envelope. It request/replies on `a2a.bridge.auth.callout.request` with JSON `BridgeMintRequest` / `BridgeMintResponse`. The in-process adapter synthesizes `ServerAuthRequestClaims` so the same `CalloutDispatcher` logic runs. Rationale: the bridge is not a NATS client connection subject to `$SYS.REQ.USER.AUTH`; only the server-facing subscriber needs the real wire format.

## Signing key custody

Inner User JWTs (`nats.jwt`) are signed via a pluggable `SigningKeySource`. Select custody with `AUTH_CALLOUT_SIGNING_KEY_SOURCE`:

| Value | Use | Notes |
|-------|-----|--------|
| `env` | Local dev only | Default. Reads `AUTH_CALLOUT_SIGNING_SECRET` (+ optional `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS`). Logs a one-time warning that env secrets are not for production. |
| `file` | Production / VMs | Requires `AUTH_CALLOUT_SIGNING_KEY_PATH` (current key material). Optional `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH` for overlap during rotation. Files are raw PEM or shared-secret bytes (no JSON envelope). |
| `vault` | Planned | Not wired yet; process exits if selected. Future work will load keys from an operator vault behind a feature flag. |

Minted User JWTs carry a `kid` claim (and matching JWS header `kid`) set to the key version string (`current` / `previous` for file and env sources). Verifiers should select the decoding key from the source's `accepted()` list using `kid`, falling back to trial verification only when `kid` is absent.

### Rotation procedure

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

Structured audit logs on denial include `reason_category`, `server_id`, `caller_id_hint` (untrusted `user_nkey`), and the full internal error for operators. Wire categories never include OIDC, x509, or verifier exception text.

## Environment variables

| Variable | Required | Purpose |
|----------|----------|---------|
| `NATS_URL` | no (default `nats://localhost:4222`) | NATS connection for the callout subscriber. |
| `AUTH_CALLOUT_SERVER_NKEY_PUBLIC` | **yes** | Public NKey of `authorization.auth_callout.issuer` — verify server-signed requests. |
| `AUTH_CALLOUT_ISSUER_NKEY_SEED` | **yes** | Seed for the callout account signing NKey — sign authorization **response** JWTs. |
| `AUTH_CALLOUT_XKEY_SEED` | no | Account XKey seed when `auth_callout.xkey` is set on the server. |
| `AUTH_CALLOUT_SERVER_XKEY_PUBLIC` | when encryption enabled | Server **persistent** XKey public key (`nats-server` seals requests with this keypair). |
| `AUTH_CALLOUT_ALLOWED_ACCOUNTS` | **yes** | Comma-separated tenant account names the resolver may mint for. |
| `AUTH_CALLOUT_SIGNING_KEY_SOURCE` | no (default `env`) | `env` \| `file` \| `vault` — selects signing key custody. |
| `AUTH_CALLOUT_SIGNING_SECRET` / `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS` | dev fallback | HS256 secret(s) for inner user JWT (`nats.jwt`) when source is `env`. |
| `AUTH_CALLOUT_SIGNING_KEY_PATH` / `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH` | when source is `file` | Key file paths for the current and previous keys. |
| `AUTH_CALLOUT_USER_JWT_TTL_SECS` | no | TTL for minted user JWT (default 300). |
| `AUTH_CALLOUT_OIDC_*` / `AUTH_CALLOUT_MTLS_*` | per verifier | See credential modules. |

Full env reference: [`A2A_RUNTIME_ENV.md`](./A2A_RUNTIME_ENV.md).

## Sample `nats-server` config (centralized)

Committed reference (placeholders, runnable after bootstrap substitutions): [`scripts/a2a-auth-callout-nats-server.conf`](../scripts/a2a-auth-callout-nats-server.conf). Keys and env block: [`scripts/a2a-auth-callout-bootstrap.sh`](../scripts/a2a-auth-callout-bootstrap.sh). Deployment units: [`scripts/a2a-auth-callout.service`](../scripts/a2a-auth-callout.service), [`scripts/a2a-auth-callout-compose.yaml`](../scripts/a2a-auth-callout-compose.yaml).

```hcl
accounts {
  AUTH: {
    users: [ { user: auth, password: auth } ]
  }
  APP: {}
  SYS: {}
}
system_account: SYS

authorization {
  auth_callout {
    issuer: ABJHLOVMPA4CI6R5KLNGOB4GSLNIY7IOUPAJC4YFNDLQVIOBYQGUWVLA
    auth_users: [ auth ]
    account: AUTH
    # Recommended in production:
    # xkey: XAMHJVPKHHPYZQQM2IVWXKJH36KDDZZMSJ32QKSQBUODFX4I4HARO4GL
  }
}
```

Generate production keys with `nsc generate nkey --account` and `nsc generate nkey --curve` — do not reuse example keys from NATS docs.

## Open questions

- **Inner user JWT**: responses currently embed the HS256 `UserJwtClaims` mint via `SigningKeySource`. A real deployment must mint **NATS User JWTs** (nkey-signed, with `pub`/`sub` permissions) before `nats-server` will accept `nats.jwt` in production (tracked in `A2A_AUTH_CALLOUT_TODO.md` #4 and integration test #8).
- **Operator mode**: multi-account `issuer_account` and signing-key scoping are not implemented in this service (single-tenant centralized model per `A2A_PLAN.md`).

## Related

- Runtime env reference: [`A2A_RUNTIME_ENV.md`](./A2A_RUNTIME_ENV.md)
- Build tracker: [`A2A_AUTH_CALLOUT_TODO.md`](../A2A_AUTH_CALLOUT_TODO.md)
