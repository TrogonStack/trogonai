# A2A Auth Callout — Build TODO

Tracker for the work needed to turn `a2a-auth-callout` from a library + stub binary into a deployable callout service that mints Account-bound User JWTs on `$SYS.REQ.USER.AUTH`.

Design context: [`docs/A2A_AUTH_CALLOUT_SKETCH.md`](./docs/A2A_AUTH_CALLOUT_SKETCH.md). High-level slot: [`A2A_TODO.md`](./A2A_TODO.md) Phase 0 — perimeter & catalog.

## Why

Today the gateway derives caller identity from `X-A2a-Spicedb-Principal` / `X-A2a-Caller-Id` request headers (with header-trust fallback). With a deployed callout, identity comes from a verified User JWT minted on connect, header-trust drops to a labs-only flag, and these downstream items unblock:

- JWT-derived `caller_id` on gateway decision-site audits (live in production)
- Populated `caller_id` segments on `A2A_PUSH_DLQ` subjects (today's `_` fallback retires)
- `A2A_BRIDGE_TRANSPORT=nats` production wiring (re-mint per HTTPS request)
- Removal of the `caller_id` header trust boundary entirely

## Current code surface (in tree)

- `rsworkspace/crates/a2a-auth-callout/src/subscriber.rs` — NATS subscriber loop on `$SYS.REQ.USER.AUTH`; deserializes requests, delegates to `AuthDispatcher`, publishes reply.
- `rsworkspace/crates/a2a-auth-callout/src/credentials/{oidc,mtls,api_key}.rs` — verifier library: OIDC JWKS, mTLS x509 chain, HMAC-SHA256 API-key.
- `rsworkspace/crates/a2a-auth-callout/src/jwt.rs` — `UserJwtClaims` minter with `AccountName`, `AudienceAccount`, `SpiceDbPrincipal`, `SpiceDbSubject` value objects.
- `rsworkspace/crates/a2a-auth-callout/src/dispatcher.rs` — `AuthDispatcher` trait; `AuthCalloutRequest` / `AuthCalloutResponse` shapes flagged `TODO(spec)`.
- `rsworkspace/crates/a2a-auth-callout/src/main.rs` — binary entry point wired to a `StubDispatcher` whose `dispatch` is `unimplemented!()`.

## Work units

### 1. Real `AuthDispatcher` impl
- [ ] Replace `StubDispatcher` in `main.rs` with a real impl that picks a verifier from `AuthCalloutRequest` shape (OIDC by bearer JWT, mTLS by client TLS chain, API-key by header).
- [ ] Call the matching `credentials::*` verifier, surface verified principal as a value object (not raw strings).
- [ ] Invoke `UserJwtClaims::mint` with the resolved `AccountName`, `SpiceDbPrincipal`, and signing key.
- [ ] Unit tests against each credential path via trait-mocked verifiers.

### 2. Account resolver
- [ ] New `account_resolver` module: trait `AccountResolver` with `resolve(principal) -> Result<AccountName, AuthCalloutError>`.
- [ ] `StaticAccountResolver` reading a config-driven mapping (env: `AUTH_CALLOUT_ACCOUNT_MAP`, format TBD — likely `<sub-claim-prefix>=<account>,…`).
- [ ] Value-object newtypes; no primitive obsession on the mapping inputs.
- [ ] Unit tests for hit / miss / default cases.

### 3. Caller-id derivation
- [ ] Lift today's `CallerId::from_principal` derivation into the callout so the minted JWT carries a `caller_id` custom claim derived from `SpiceDbPrincipal.spicedb_subject`.
- [ ] Ensure derivation is deterministic per tenant Account (stable `_INBOX.{caller_id}.>` ACL across reconnects).

### 4. Subject ACL minting
- [ ] Extend `UserJwtClaims` (or add an `IssuedPermissions` value object) to carry a NATS permissions block bounding the User to:
  - publish on `a2a.gateway.>`
  - subscribe on `_INBOX.{caller_id}.>`
  - subscribe on `a2a.push.{caller_id}.>`
- [ ] Cross-check against the role templates in `scripts/acl-templates/` so the minted permissions match the deployed ACL posture.

### 5. Signing-key custody
- [ ] Replace the dev `AUTH_CALLOUT_SIGNING_SECRET` env fallback in `main.rs` with a `SigningKeySource` trait: `FileSource` (path), `EnvSource` (raw secret — dev only, log a warn once), pluggable `VaultSource` stub.
- [ ] Support key versioning so rotation can prefer the new key on mint while accepting the old one during an overlap window.
- [ ] Document expected secret-store integration points in `docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md` (new).

### 6. Wire format pin (NATS auth-callout extension)
- [ ] Replace the illustrative `AuthCalloutRequest` / `AuthCalloutResponse` structs with the real NATS server wire format: NKey-signed envelope + JWT-encoded inner payload per the auth-callout extension.
- [ ] Decode the server-signed request JWT using the server's configured xkey.
- [ ] Encode the response as a callout-signed JWT the server can verify.
- [ ] Pin against a specific `nats-server` minor version in tests; record the version in `docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md`.

### 7. Denial encoding
- [ ] `subscriber.rs::publish_denial` today emits `{"error":"authorization denied"}`. Replace with the auth-callout extension's denial format (signed JWT carrying the deny indicator).
- [ ] Make sure denial paths still preserve trace context for audit.

### 8. Integration test
- [ ] Testcontainer-backed `nats-server` configured with `authorization { auth_callout { ... } }` pointing at this subscriber.
- [ ] Mock OIDC issuer (reuse the pattern from existing OIDC tests if present, otherwise inline).
- [ ] Connect a NATS client with an OIDC bearer token; assert the server admits the connection, the User JWT carries the expected `aud` and `caller_id`, and the permissions block bounds publish/subscribe correctly.
- [ ] Repeat for mTLS and API-key paths.
- [ ] Mark live tests `#[ignore]` so CI can skip without infra.

### 9. Gateway consumption path
- [ ] Add a code path in `a2a-gateway` that reads the verified principal from connection metadata (when async-nats exposes it) and prefers it over the `X-A2a-Spicedb-Principal` header.
- [ ] Gate the header-trust fallback behind a new env (`A2A_GATEWAY_TRUST_CALLER_HEADERS`, default off in prod). Surface a warn-once when the fallback is active.
- [ ] Update `resolve_gateway_caller_identity` tests for both paths.

### 10. Bridge consumption path
- [ ] In `a2a-bridge`, re-mint a per-request User JWT against the callout when `A2A_BRIDGE_TRANSPORT=nats` is live, instead of using the stub identity.
- [ ] Cover via the existing `a2a-bridge::nats_transport_harness`; add a callout-mock dispatcher for unit tests.

### 11. Docs
- [ ] Promote `docs/A2A_AUTH_CALLOUT_SKETCH.md` from "sketch" to "design" once wire format is pinned; rename if appropriate.
- [ ] Add `docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md` — runbook covering NSC operator/account provisioning, callout service NATS config block, signing-key custody, rotation procedure.
- [ ] Update `docs/A2A_RUNTIME_ENV.md` for new `AUTH_CALLOUT_*` envs.

### 12. Operator artifacts
- [ ] NSC operator/account JWT provisioning script (or extension of the existing bootstrap script).
- [ ] Reference NATS server config snippet committed under `scripts/` showing the callout binding for a single tenant Account.
- [ ] Deployment manifest matching the rest of the stack (k8s / systemd — match the in-tree pattern).

## Suggested ordering

1. **#1 + #2 + #3 + #4** — non-stub binary with trait-mocked verifiers, real Account resolver, real caller-id derivation, real permissions block. Lands as a runnable callout against the illustrative wire format; unit-tested end-to-end.
2. **#5** — signing-key custody so the binary can mint with operator-loaded material instead of the dev env fallback.
3. **#6 + #7** — pin the wire format and denial encoding against a specific NATS server version. Gated on picking the target version with operators.
4. **#8** — testcontainer integration test exercising the real wire format end-to-end.
5. **#9 + #10** — gateway + bridge consumption paths flip to JWT-derived caller identity; header-trust fallback retires to a labs flag.
6. **#11 + #12** — docs + deployment artifacts for production rollout.

## Out of scope

- Org-specific IdP integration beyond OIDC JWKS / mTLS x509 / HMAC API-key (already shipped in `credentials/`).
- SpiceDB principal schema — owned by org standards, callout only carries it as `data` claim.
- Multi-Operator NATS topologies — single-Operator assumption per `A2A_PLAN.md` §Tenancy model.
- Token introspection caching strategy — defer until the OIDC verifier shows latency in production traffic.
