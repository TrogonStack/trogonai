# A2A Auth Callout — Build TODO

Remaining work to deploy `a2a-auth-callout` as a production callout service on `$SYS.REQ.USER.AUTH`.

Design context: [`docs/A2A_AUTH_CALLOUT_SKETCH.md`](./docs/A2A_AUTH_CALLOUT_SKETCH.md). Operator runbook: [`docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md`](./docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md). High-level slot: [`A2A_TODO.md`](./A2A_TODO.md) Phase 0 — perimeter & catalog.

## Why

Today the gateway still derives caller identity from the `X-A2a-Spicedb-Principal` / `X-A2a-Caller-Id` request headers with a header-trust fallback. Once a callout is deployed and the gateway reads the JWT-derived principal off the connection, these downstream items unblock:

- JWT-derived `caller_id` on gateway decision-site audits (live in production)
- Populated `caller_id` segments on `A2A_PUSH_DLQ` subjects (today's `_` fallback retires)
- `A2A_BRIDGE_TRANSPORT=nats` production wiring (re-mint per HTTPS request)
- Removal of the `caller_id` header-trust boundary entirely

## Current code surface (in tree)

- `rsworkspace/crates/a2a-auth-callout/src/subscriber.rs` — NATS subscriber loop on `$SYS.REQ.USER.AUTH`; decodes the server-signed authorization request, delegates to `CalloutDispatcher`, publishes a signed authorization response (success or denial) via `AuthCalloutWireCodec`.
- `rsworkspace/crates/a2a-auth-callout/src/wire/` — pinned NATS auth-callout wire format: NKey-signed request/response JWTs, optional XKey envelope encryption, in-process `BridgeMintAdapter`.
- `rsworkspace/crates/a2a-auth-callout/src/credentials/{oidc,mtls,api_key}.rs` — OIDC JWKS, mTLS x509 chain, HMAC-SHA256 API-key verifiers.
- `rsworkspace/crates/a2a-auth-callout/src/jwt.rs` — `UserJwtClaims` minter (Account-bound `aud`, `SpiceDbPrincipal` `data`, derived `caller_id`, `IssuedPermissions` `nats_permissions`, `kid` rotation header) producing a `MintedUserJwt` newtype.
- `rsworkspace/crates/a2a-auth-callout/src/dispatcher.rs` — `CalloutDispatcher` routes OIDC bearer / mTLS client cert / API-key credential off `ServerAuthRequestClaims`, resolves the tenant Account, mints via the active `SigningKeySource` handle.
- `rsworkspace/crates/a2a-auth-callout/src/account_resolver.rs` — `AccountResolver` trait + env-driven `StaticAccountResolver` (`AUTH_CALLOUT_ALLOWED_ACCOUNTS`).
- `rsworkspace/crates/a2a-auth-callout/src/permissions.rs` — `IssuedPermissions::default_for_caller` mirrors `scripts/acl-templates/caller.acl`.
- `rsworkspace/crates/a2a-auth-callout/src/signing_key_source/` — `env` / `file` / `vault`-stub custody with `KeyVersion` overlap rotation and `kid`-aware verification.
- `rsworkspace/crates/a2a-auth-callout/src/denial_category.rs` — opaque denial categories carried in the signed denial JWT.
- `rsworkspace/crates/a2a-auth-callout/src/main.rs` — binary entry point assembling signing-key source, NKey/XKey wire codec, account resolver, and credential verifiers into a live `Subscriber`.

## Remaining work

### 8. Integration test against a real `nats-server`
- [x] Testcontainer-backed `nats-server` (pinned to 2.14.x — see `docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md`) configured with `authorization { auth_callout { ... } }` pointing at this subscriber.
- [x] Mock OIDC issuer (inline JWKS endpoint).
- [x] Connect a NATS client with an OIDC bearer token; assert the minted User JWT carries the expected `aud`, stable `caller_id`, and `IssuedPermissions` matching `scripts/acl-templates/caller.acl`. *(Connect admission is still blocked on NATS User JWT mint — see `TODO(CALLOUT_E2E_CONNECT_ADMISSION)` in `tests/nats_server_callout_integration.rs`.)*
- [x] Repeat for the mTLS and API-key credential paths.
- [x] Mark live tests `#[ignore = "requires Docker (task #8)"]` so CI can skip without infra.

### 9. Gateway consumption path
- [ ] In `a2a-gateway`, read the verified principal from the NATS connection metadata (once `async-nats` exposes it) and prefer it over the `X-A2a-Spicedb-Principal` header. *(Blocked: `async-nats` does not yet surface the auth-callout-minted JWT to clients. Seam is already in tree as `ConnectionCallerIdentitySource` — `UnavailableConnectionCallerIdentity` is the current stand-in; flip the binding when upstream lands.)*
- [x] Gate the header-trust fallback behind a new env (`A2A_GATEWAY_TRUST_CALLER_HEADERS`, default off in prod). Surface a warn-once when the fallback is active. *(`jwt_caller_identity::gateway_caller_identity_policy` + `TRUST_CALLER_HEADERS_WARN`.)*
- [x] Update `resolve_gateway_caller_identity` tests for both paths.

### 11. Docs
- [ ] Promote `docs/A2A_AUTH_CALLOUT_SKETCH.md` from "sketch" to "design" now that the wire format is pinned; rename if appropriate.
- [ ] Update `docs/A2A_RUNTIME_ENV.md` with the deployed `AUTH_CALLOUT_*` envs (server NKey/XKey, signing-key source, allowed accounts, user JWT TTL, OIDC, mTLS).

### 12. Operator artifacts
- [ ] NSC operator/account JWT provisioning script (or extension of `scripts/a2a-nsc-bootstrap.sh`) covering the callout signing keys and the AUTH/APP/SYS layout.
- [ ] Reference `nats-server` config snippet committed under `scripts/` showing the callout binding for a single tenant Account.
- [ ] Deployment manifest matching the rest of the stack (k8s / systemd — match the in-tree pattern).

## Suggested ordering

1. **#11 + #12** — promote docs and ship operator artifacts for production rollout.
2. **#4** (NATS User JWT mint) — unblocks connect-admission assertions in the #8 testcontainer suite.
3. **#9** — once `async-nats` exposes the verified JWT on the connection, bind a real `ConnectionCallerIdentitySource` and retire the header-trust default. (Tracking item only; no code work until upstream lands.)

## Out of scope

- Org-specific IdP integration beyond OIDC JWKS / mTLS x509 / HMAC API-key (already shipped in `credentials/`).
- SpiceDB principal schema — owned by org standards; the callout only carries it as the `data` claim.
- Multi-Operator NATS topologies — single-Operator assumption per `A2A_PLAN.md` §Tenancy model.
- Token introspection caching strategy — defer until the OIDC verifier shows latency in production traffic.
