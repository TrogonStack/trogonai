# A2A auth callout — deployment reference

Operational notes for the `a2a-auth-callout` service on NATS `$SYS.REQ.USER.AUTH`.

Design context: [`A2A_AUTH_CALLOUT_SKETCH.md`](./A2A_AUTH_CALLOUT_SKETCH.md). Build tracker: [`A2A_AUTH_CALLOUT_TODO.md`](../A2A_AUTH_CALLOUT_TODO.md).

## Pinned versions

| Component | Version | Notes |
|-----------|---------|--------|
| **nats-server** | **2.10.x** (validated against **2.10.29** server source) | Auth callout extension ships from [v2.10.0](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout.md). |
| **nats-jwt-rs** | **0.1.1** | Authorization request/response JWT encode/decode. |
| **nkeys** | **0.4.5** (`xkeys` feature) | NKey signing and XKey encrypt/decrypt for callout payloads. |

Spec references:

- [Auth callout configuration](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout.md)
- [`nats-server` `server/auth_callout.go`](https://github.com/nats-io/nats-server/blob/v2.10.29/server/auth_callout.go) (request/reply encoding)
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

## Environment variables

| Variable | Required | Purpose |
|----------|----------|---------|
| `NATS_URL` | no (default `nats://localhost:4222`) | NATS connection for the callout subscriber. |
| `AUTH_CALLOUT_SERVER_NKEY_PUBLIC` | **yes** | Public NKey of `authorization.auth_callout.issuer` — verify server-signed requests. |
| `AUTH_CALLOUT_ISSUER_NKEY_SEED` | **yes** | Seed for the callout account signing NKey — sign authorization **response** JWTs. |
| `AUTH_CALLOUT_XKEY_SEED` | no | Account XKey seed when `auth_callout.xkey` is set on the server. |
| `AUTH_CALLOUT_SERVER_XKEY_PUBLIC` | when encryption enabled | Server **persistent** XKey public key (`nats-server` seals requests with this keypair). |
| `AUTH_CALLOUT_ALLOWED_ACCOUNTS` | **yes** | Comma-separated tenant account names the resolver may mint for. |
| `AUTH_CALLOUT_SIGNING_SECRET` | dev fallback | HS256 secret for inner user JWT (`nats.jwt`) until NATS User JWT mint lands in task #4. |
| `AUTH_CALLOUT_USER_JWT_TTL_SECS` | no | TTL for minted user JWT (default 300). |
| `AUTH_CALLOUT_OIDC_*` / `AUTH_CALLOUT_MTLS_*` | per verifier | See credential modules. |

## Sample `nats-server` config (centralized)

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

- **Inner user JWT**: responses currently embed the HS256 `UserJwtClaims` mint from `AUTH_CALLOUT_SIGNING_SECRET`. A real deployment must mint **NATS User JWTs** (nkey-signed, with `pub`/`sub` permissions) before `nats-server` will accept `nats.jwt` in production (tracked in `A2A_AUTH_CALLOUT_TODO.md` #4 and integration test #8).
- **Operator mode**: multi-account `issuer_account` and signing-key scoping are not implemented in this service (single-tenant centralized model per `A2A_PLAN.md`).
