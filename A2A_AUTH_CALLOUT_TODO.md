# A2A Auth Callout — Build TODO

Remaining work to deploy `a2a-auth-callout` as a production callout service on `$SYS.REQ.USER.AUTH`.

Design context: [`docs/A2A_AUTH_CALLOUT_DESIGN.md`](./docs/A2A_AUTH_CALLOUT_DESIGN.md). Operator runbook: [`docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md`](./docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md). High-level slot: [`A2A_TODO.md`](./A2A_TODO.md) Phase 0 — perimeter & catalog.

## Why

Today the gateway derives caller identity from the `X-A2a-Spicedb-Principal` / `X-A2a-Caller-Id` request headers with a header-trust fallback. Once gateway clients carry the auth-callout-minted JWT as a signed message header, these downstream items unblock:

- JWT-derived `caller_id` on gateway decision-site audits (live in production)
- Populated `caller_id` segments on `A2A_PUSH_DLQ` subjects (today's `_` fallback retires)
- `A2A_BRIDGE_TRANSPORT=nats` production wiring (re-mint per HTTPS request)
- Removal of the `caller_id` header-trust boundary entirely

## Current code surface (in tree)

- `rsworkspace/crates/a2a-auth-callout/src/subscriber.rs` — NATS subscriber loop on `$SYS.REQ.USER.AUTH`; decodes the server-signed authorization request, delegates to `CalloutDispatcher`, publishes a signed authorization response (success or denial) via `AuthCalloutWireCodec`.
- `rsworkspace/crates/a2a-auth-callout/src/wire/` — pinned NATS auth-callout wire format: NKey-signed request/response JWTs, optional XKey envelope encryption, in-process `BridgeMintAdapter`.
- `rsworkspace/crates/a2a-auth-callout/src/credentials/{oidc,mtls,api_key}.rs` — OIDC JWKS, mTLS x509 chain, HMAC-SHA256 API-key verifiers.
- `rsworkspace/crates/a2a-auth-callout/src/jwt/` — NATS User JWT minter (`ed25519-nkey`, `nats.pub` / `nats.sub` from `IssuedPermissions`, `sub` = caller User NKey, Account NKey `iss`) producing a `MintedUserJwt` newtype; carries `aud`, `caller_id`, `kid`, SpiceDB `data`.
- `rsworkspace/crates/a2a-auth-callout/src/dispatcher.rs` — `CalloutDispatcher` routes OIDC bearer / mTLS client cert / API-key credential off `ServerAuthRequestClaims`, resolves the tenant Account, mints via the active `SigningKeySource` handle.
- `rsworkspace/crates/a2a-auth-callout/src/account_resolver.rs` — `AccountResolver` trait + env-driven `StaticAccountResolver` (`AUTH_CALLOUT_ALLOWED_ACCOUNTS`).
- `rsworkspace/crates/a2a-auth-callout/src/permissions.rs` — `IssuedPermissions::default_for_caller` mirrors `scripts/acl-templates/caller.acl`.
- `rsworkspace/crates/a2a-auth-callout/src/signing_key_source/` — `env` / `file` / `vault`-stub custody with `KeyVersion` overlap rotation and `kid`-aware verification.
- `rsworkspace/crates/a2a-auth-callout/src/denial_category.rs` — opaque denial categories carried in the signed denial JWT.
- `rsworkspace/crates/a2a-auth-callout/src/main.rs` — binary entry point assembling signing-key source, NKey/XKey wire codec, account resolver, and credential verifiers into a live `Subscriber`.

## Remaining work

### Gateway per-message caller identity — signed-header convention

The NATS protocol authenticates the publishing **connection** at connect time but does not stamp publisher identity onto MSG / HMSG frames delivered to subscribers. A client library can only surface what the server sends; there is no per-message principal field to expose. The gap is structural — no client-side change can close it.

**Decision (landed).** Publishers attach the auth-callout-minted User JWT (already operator-signed, short-lived, audience-bound, carries `SpiceDbPrincipal` in `data`) as a NATS header on every publish to `a2a.gateway.>`. Gateway verifies signature + expiry + audience against the same `kid`-aware `SigningKeySource` rotation the callout uses. Rejected alternatives: a trusted-ingress rebroadcaster (doesn't cover NATS-native publishers), and request/reply-only routing (loses `message/stream` + `tasks/resubscribe`).

**Work:**
- [ ] Define the signed-header contract: header name, JWT shape (reuse `MintedUserJwt`), gateway-side verifier crate boundary.
- [ ] Implement `MessageCallerIdentitySource` (rename of `ConnectionCallerIdentitySource`): "verify the JWT header on this `async_nats::Message`." Verifier lives in the gateway crate; signing-key material reuses `a2a-auth-callout`'s `SigningKeySource`.
- [ ] Wire the new source into `resolve_gateway_caller_identity` (`rsworkspace/crates/a2a-gateway/src/jwt_caller_identity.rs:129`); delete `UnavailableConnectionCallerIdentity` (`rsworkspace/crates/a2a-gateway/src/runtime.rs:292`) and the connection-wins branch.
- [ ] Update the bridge / agent publish paths to attach the header on outbound publishes to `a2a.gateway.>`.
- [ ] Retire `A2A_GATEWAY_TRUST_CALLER_HEADERS` from production env once the new path is live.

## Out of scope

- Org-specific IdP integration beyond OIDC JWKS / mTLS x509 / HMAC API-key (already shipped in `credentials/`).
- SpiceDB principal schema — owned by org standards; the callout only carries it as the `data` claim.
- Multi-Operator NATS topologies — single-Operator assumption per `A2A_PLAN.md` §Tenancy model.
- Token introspection caching strategy — defer until the OIDC verifier shows latency in production traffic.
