# Trogon Identity and Trust -- Version 1.0

> **Status:** Draft (descriptive, not aspirational) - **Date:** 2026-07-02
>
> This specification documents the identity, authentication, and
> proof-of-possession guarantees that `a2a-auth-callout` and
> `trogon-aauth-verify` **already implement in code** as of this writing.
> It is not a design proposal: every MUST below is backed by a citation
> to the crate, module, and (where useful) function that enforces it. If
> a future code change removes a guarantee described here, this document
> is out of date and must be revised, not treated as authoritative over
> the code.

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT",
"SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this
document are to be interpreted as described in
[RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119) and
[RFC 8174](https://datatracker.ietf.org/doc/html/rfc8174).

---

## Table of Contents

1. [Scope](#1-scope)
2. [Terminology](#2-terminology)
3. [NATS Auth-Callout Connection Authentication](#3-nats-auth-callout-connection-authentication)
4. [Credential Scheme Selection](#4-credential-scheme-selection)
5. [OIDC Verification](#5-oidc-verification)
6. [mTLS Verification](#6-mtls-verification)
7. [Minted NATS User JWT](#7-minted-nats-user-jwt)
8. [Issued NATS Permissions](#8-issued-nats-permissions)
9. [Opaque Denial Responses](#9-opaque-denial-responses)
10. [AAuth Token Verification](#10-aauth-token-verification)
11. [NATS Proof of Possession](#11-nats-proof-of-possession)
12. [Replay Protection](#12-replay-protection)
13. [Failure Semantics](#13-failure-semantics)
14. [Security Considerations](#14-security-considerations)
15. [Non-Guarantees](#15-non-guarantees)
16. [Conformance Checklist](#16-conformance-checklist)
17. [Worked Examples](#17-worked-examples)
18. [References](#18-references)

---

## 1. Scope

This specification covers two Rust crates in `rsworkspace/crates/`:

- **`a2a-auth-callout`** (binary `a2a-auth-callout`, library `a2a_auth_callout`):
  implements the NATS server's `$SYS.REQ.USER.AUTH` auth-callout protocol.
  It authenticates an incoming NATS *connection* and mints a scoped NATS
  user JWT embedding publish/subscribe permissions.
- **`trogon-aauth-verify`**: implements server-side verification for
  `draft-hardt-aauth-protocol` ("AAuth") bearer tokens and an
  RFC-9421-inspired proof-of-possession (PoP) scheme adapted for NATS
  messages. This authenticates an individual *agent identity* making a
  request, which may be layered on top of an already-established NATS
  connection from the callout above.

Wire types shared by both crates but owned elsewhere are cited where
relevant: `a2a-identity-types` (NATS auth-callout wire identity types)
and `trogon-identity-types::aauth` (AAuth wire types, transport-agnostic).

Out of scope: `a2a-redaction` (payload redaction), `trogon-aauth-sdk`
(agent-side signer), `trogon-aauth-person` (JetStream-KV-backed replay
store and Person Server), and SpiceDB-side authorization. These are
referenced only where necessary to describe a trust boundary.

---

## 2. Terminology

| Term | Definition |
| --- | --- |
| **Auth callout** | The NATS server's `$SYS.REQ.USER.AUTH` extension point; `a2a-auth-callout` answers these requests to authenticate connections. |
| **User JWT** | The NATS v2 user JWT minted by the callout on success, embedding NATS ACL permissions, returned to the connecting client via the NATS server. |
| **DenialCategory** | A closed, opaque enum returned on the wire when a connection is denied; never carries verification detail. |
| **AAuth** | `draft-hardt-aauth-protocol`, a proof-of-possession token scheme with three JWT `typ` values (`aa-agent+jwt`, `aa-resource+jwt`, `aa-auth+jwt`). |
| **PoP** | Proof of possession: a signature over a canonical request representation proving the signer holds the private key bound to a `cnf.jwk` claim. |
| **`cnf.jwk`** | The public-key confirmation claim inside an `aa-agent+jwt`, binding the agent identity to a key pair it holds privately. |
| **`jkt`** | JWK thumbprint, used as the `keyid` in the canonical signature base. |
| **Canonical base** | The fixed-format string constructed by `NatsSignatureEnvelope::canonical_base` that both signer and verifier must agree on byte-for-byte. |
| **Replay store** | The `ReplayStore` trait and its `check_and_insert` operation, used to reject re-submission of a previously accepted PoP nonce. |
| **Skew** | Absolute difference between the verifier's clock and the `AAuth-Sig-Created` timestamp on a signed request. |

---

## 3. NATS Auth-Callout Connection Authentication

### 3.1 Subscription and worker pool

The callout server MUST subscribe to `$SYS.REQ.USER.AUTH` using a queue
subscription rather than a plain subscription.

- Evidence: `rsworkspace/crates/a2a-auth-callout/src/subscriber.rs`,
  `Subscriber::run`, calls `self.client.queue_subscribe(AUTH_CALLOUT_SUBJECT, queue)`.
  `AUTH_CALLOUT_SUBJECT = "$SYS.REQ.USER.AUTH"`.
- The queue group name MUST default to `"a2a-auth-callout"` and MUST be
  overridable via the `AUTH_CALLOUT_QUEUE_GROUP` environment variable.
  Evidence: same file, `let queue = ReadEnv::var(env, "AUTH_CALLOUT_QUEUE_GROUP").unwrap_or_else(|_| "a2a-auth-callout".to_string());`
- Rationale documented in code: a plain subscription would deliver each
  connect request to every replica, causing all of them to race to
  publish a reply to the same inbox.

### 3.2 Wire decoding

Every inbound auth-callout request MUST be decoded through
`AuthCalloutWireCodec::decode_request` (`src/wire/wire_codec.rs`), which
unwraps the NATS-standard signed/optionally-encrypted authorization
request envelope and verifies the server's issuer Nkey.

- If the request cannot be decoded, the subscriber MUST publish an
  empty payload to the reply subject rather than leaving the client to
  stall until the NATS server's own timeout. Evidence:
  `subscriber.rs`, the `Err(e)` arm of `self.wire.decode_request(...)`
  logs a warning and calls `self.client.publish(reply.clone(), Vec::new().into())`.
- If publishing the success or denial reply itself fails, the
  subscriber MUST fall back to publishing an empty payload for the
  same reason. Evidence: the `if let Err(pub_err) = publish_result`
  branch at the end of the spawned task in `subscriber.rs`.

### 3.3 Encryption is all-or-nothing

If Xkey-based payload encryption is configured, both `account_xkey_seed`
and `server_xkey_public` MUST be present; a half-configured pair MUST be
rejected at construction rather than silently operating unencrypted or
panicking at request time. (Evidence: `src/wire/wire_codec.rs`
construction path, per the crate's documented invariant that encryption
configuration is validated eagerly and is both-or-neither.)

---

## 4. Credential Scheme Selection

### 4.1 Fixed preference order

The dispatcher MUST select exactly one credential scheme per request
using a fixed preference order: **OIDC, then mTLS, then API key**. The
first scheme whose corresponding request field is present is selected;
schemes are not combined.

- Evidence: `rsworkspace/crates/a2a-auth-callout/src/dispatcher.rs`,
  `CalloutDispatcher::select_scheme`:

  ```rust
  fn select_scheme(&self, request: &ServerAuthRequestClaims) -> Result<AuthScheme, AuthCalloutError> {
      if request.connect_opts_jwt().is_some() || request.connect_opts_opaque_pass().is_some() {
          return Ok(AuthScheme::Oidc);
      }
      if request.primary_client_cert().is_some() {
          return Ok(AuthScheme::MTls);
      }
      if request.connect_opts_auth_token().is_some() {
          return Ok(AuthScheme::ApiKey);
      }
      Err(CredentialError::InvalidRequest("no credential material in authorization request".into()).into())
  }
  ```

- If no credential material of any recognized shape is present, the
  dispatcher MUST return `CredentialError::InvalidRequest` (mapped to
  `DenialCategory::InvalidRequest`, see Section 9).
- The `ApiKey` scheme is marked `#[allow(deprecated)]` at every call
  site in `dispatcher.rs`; it is a transitional scheme, not the
  steady-state path.

### 4.2 Verifier unavailability

If the selected scheme's verifier (`oidc`, `mtls`, or `api_key` on
`CalloutDispatcherConfig`) is not configured, the dispatcher MUST return
`CredentialError::VerifierUnavailable { scheme }` naming the scheme
(`"OIDC"`, `"mTLS"`, or `"API-key"`). Evidence: `dispatcher.rs`,
the `.ok_or(...)` calls in each `AuthScheme` match arm.

---

## 5. OIDC Verification

Evidence: `rsworkspace/crates/a2a-auth-callout/src/credentials/oidc.rs`.

### 5.1 Discovery hardening

`JwksOidcVerifier::discover` MUST enforce all of the following when
performing OIDC discovery:

1. A 10-second total request timeout and a 5-second connect timeout.
   Evidence: `reqwest::Client::builder().timeout(Duration::from_secs(10)).connect_timeout(Duration::from_secs(5))`.
2. HTTP redirects MUST be disabled (`redirect(reqwest::redirect::Policy::none())`).
3. The discovery document's `issuer` field MUST match the configured
   issuer URL (trailing slash normalized). A mismatch MUST be rejected
   with `CredentialError::InvalidCredentials`.
4. The discovered `jwks_uri` MUST be same-origin (scheme, host, and
   port, with default-port normalization for `https:443`/`http:80`) as
   the configured issuer. A cross-origin `jwks_uri` MUST be rejected.
   Evidence: `same_origin(jwks_uri, issuer.as_str())` gate before the
   verifier is constructed.

The purpose stated in code comments is to prevent a tampered or
MITM'd discovery document from redirecting JWKS fetches to an
attacker-controlled host after the issuer check has passed.

### 5.2 RSA-only JWK support

`JwksOidcVerifier::decoding_key_for_jwk` MUST accept only
`AlgorithmParameters::RSA` JWKs. Any other JWK algorithm parameter
(including EC or OKP) MUST be rejected with
`CredentialError::InvalidCredentials("OIDC JWK must be RSA for this verifier")`.

```rust
fn decoding_key_for_jwk(jwk: &jsonwebtoken::jwk::Jwk) -> Result<DecodingKey, AuthCalloutError> {
    match &jwk.algorithm {
        AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e)...,
        _ => Err(... "OIDC JWK must be RSA for this verifier" ...),
    }
}
```

This is a real, current restriction: an identity provider rotating to
EC or OKP keys for its OIDC signing JWKs will break token verification
on this path (see Section 15, Non-Guarantees).

### 5.3 Token validation

`verify_internal` MUST:

- Reject the request if no expected audiences are configured
  (`expected_id_token_audiences.is_empty()`).
- Require a `kid` in the JWT header and resolve the matching JWK from
  the fetched JWK set by `kid`; a missing `kid` or no matching JWK MUST
  be rejected.
- Validate signature, `iss` (set to the configured issuer), and `aud`
  (set to the configured expected audiences) via `jsonwebtoken::Validation`.
- Require a `sub` claim, parsed through `ExternalSubject::new`, which
  performs its own validation (see `a2a-identity-types`).

### 5.4 Caller identity derivation

On success, the verifier MUST derive a `CallerId` via `derive_caller_id(sub_str, account)`
and attach `IssuedPermissions::default_for_caller(&caller_id)` (Section 8)
to the returned `UserJwtClaims`.

---

## 6. mTLS Verification

Evidence: `rsworkspace/crates/a2a-auth-callout/src/credentials/mtls.rs`.

### 6.1 Chain parsing

`X509MtlsVerifier::verify_sync` MUST parse the client-presented PEM as
an ordered chain (leaf first, then any intermediates) and MUST reject
a PEM bundle containing no `CERTIFICATE` block.

### 6.2 Validity window

The leaf certificate's validity window MUST cover the verification
time (`leaf.validity().is_valid_at(ASN1Time::from(now))`); otherwise
verification MUST fail with `CredentialError::InvalidCredentials`.

### 6.3 End-entity constraint (RFC 5280)

A leaf certificate whose `basicConstraints` extension has `cA = true`
MUST be rejected as "expected end-entity, got CA". Absence of the
`basicConstraints` extension is treated as end-entity (permitted).

```rust
if let Ok(Some(bc)) = leaf.basic_constraints()
    && bc.value.ca
{
    return Err(... "client certificate is a CA, expected end-entity" ...);
}
```

### 6.4 Chain-of-trust walk

The verifier MUST walk from the leaf through supplied intermediates
toward a configured trust anchor (`TrustAnchorPem`), at each step
requiring the candidate issuer to be currently valid
(`c.validity().is_valid_at(asn1_now)`) and to have produced a verifiable
signature over the current certificate
(`current.verify_signature(Some(&c.tbs_certificate.subject_pki)).is_ok()`).
The walk MUST be bounded to `chain.len() + 1` iterations. A chain that
does not terminate at a configured anchor MUST be rejected with
`"client certificate does not chain to a configured trust anchor"`.

Revocation (CRL/OCSP) checking is not part of this walk (see Section 15).

### 6.5 Subject extraction

The caller's external subject MUST be derived from the leaf
certificate's DN (`leaf.subject().to_string()`); if the DN is empty,
the verifier MUST fall back to a DER-derived encoding via
`external_subject_from_der("mtls", leaf_der)` rather than failing.

---

## 7. Minted NATS User JWT

Evidence: `rsworkspace/crates/a2a-auth-callout/src/jwt/nats_user_jwt.rs`.

### 7.1 Header

Minted user JWTs MUST use header `"typ": "JWT"` and
`"alg": "ed25519-nkey"` (`HEADER_TYPE`, `HEADER_ALGORITHM` constants).
Verification (`verify_with_material`) MUST reject any token whose
header does not match both values exactly.

### 7.2 `jti` computation

The `jti` claim MUST be computed as the SHA-512/256 digest
(`Sha512_256`) of the serialized claim body, Base32-no-padding encoded,
following NATS' own JWT convention: `iss` MUST be populated in the
claim body *before* hashing, and `jti` MUST be the empty string during
that hash computation, then substituted into the final signed payload
afterward.

```rust
let payload_template = NatsUserJwtPayload { /* iss set, jti: String::new() */ ... };
let encoded_claim = serde_json::to_string(&payload_template)...;
let mut hasher = Sha512_256::new();
hasher.update(encoded_claim.as_bytes());
let jti = BASE32_NOPAD.encode(&hasher.finalize());
```

### 7.3 Signing

The signing input MUST be `"{header_b64}.{claims_b64}"` (both segments
URL-safe base64, no padding) and MUST be signed with an Ed25519 keypair
sourced from a `SigningKeySource` (`MintingMaterial::issuer_keypair()`).
The `aud` claim MUST be non-empty; an empty account name MUST be
rejected before signing (`"account name must be non-empty"`).

### 7.4 Freshness fields

Minted tokens MUST carry `iat` (mint time), `exp` (`iat + ttl`), and
`nbf` (equal to `iat`). `MintedUserJwt::ensure_fresh()`
(`a2a-identity-types/src/jwt.rs`) MUST decode the payload without
verifying the signature and MUST reject the token if `exp <= now` or if
`nbf` is present and `nbf > now`. This is a client-side freshness check,
distinct from server-side signature verification.

### 7.5 Wire transport hygiene

`CallerJwtHeaderValue` (carried on the `A2a-Caller-Jwt` header) and
`MintedUserJwt` MUST both validate that the token is a 3-segment,
non-empty-segment compact JWT at construction time, without verifying
the signature. `CallerJwtHeaderValue::fmt::Display` MUST render the
literal string `"<redacted>"` regardless of the wrapped token, so that
accidental `{}`/`tracing` formatting of the value cannot leak the raw
token into logs.

---

## 8. Issued NATS Permissions

Evidence: `rsworkspace/crates/a2a-auth-callout/src/permissions.rs`.

### 8.1 Subject pattern validation

Every `SubjectPattern` (used in `IssuedPermissions.publish_allow` /
`subscribe_allow`) MUST be non-empty and MUST NOT contain any
whitespace character. This validation MUST run both on
construction (`SubjectPattern::new`) and again when a `SubjectPattern`
is deserialized from JSON (a hand-written `Deserialize` impl, not
derived, specifically so permissions embedded in a previously-minted
JWT are re-validated rather than trusted as opaque strings).

### 8.2 Default caller ACL

`IssuedPermissions::default_for_caller(caller_id)` MUST grant exactly:

| Direction | Subject pattern |
| --- | --- |
| Publish | `a2a.gateway.>` |
| Subscribe | `_INBOX.{caller_id}.>` |
| Subscribe | `a2a.push.{caller_id}.>` |

```rust
pub fn default_for_caller(caller_id: &CallerId) -> Self {
    let inbox = format!("_INBOX.{}.>", caller_id.as_str());
    let push = format!("a2a.push.{}.>", caller_id.as_str());
    Self {
        publish_allow: vec![SubjectPattern::new("a2a.gateway.>").expect("static literal")],
        subscribe_allow: vec![
            SubjectPattern::new(inbox).expect("derived from validated caller_id"),
            SubjectPattern::new(push).expect("derived from validated caller_id"),
        ],
    }
}
```

This is the sole default ACL: a caller reaches only the gateway ingress
subject and its own reply/push subjects, never another caller's inbox
or push subject.

### 8.3 `CallerId` constraints

`CallerId` (`a2a-identity-types/src/caller.rs`) MUST reject an empty
string and MUST reject any character that is `.`, `*`, `>`, or
whitespace, since a `CallerId` is interpolated directly into NATS
subject segments in Section 8.2's templates.

### 8.4 Custom ACL templates

`SubjectAclTemplate::materialize` supports `{caller}`, `{aud}`, `{sub}`,
and `{iss}` placeholders. Any other placeholder name MUST be rejected
(`TemplateError::UnknownPlaceholder`). Any placeholder value that is
empty or contains `.`, `*`, `>`, or whitespace MUST be rejected
(`TemplateError::InvalidPlaceholderValue`) before being interpolated,
closing an injection path where a malicious claim value could otherwise
widen the rendered subject pattern's scope.

---

## 9. Opaque Denial Responses

Evidence: `rsworkspace/crates/a2a-auth-callout/src/denial_category.rs`,
`src/denial_reason.rs`, `src/denial_claims.rs`, `src/subscriber.rs`.

### 9.1 Closed category enum

`DenialCategory` MUST be exactly these six variants, with these exact
`as_str()` wire values, and no others:

| Variant | Wire value (`as_str()`) |
| --- | --- |
| `InvalidCredentials` | `invalid_credentials` |
| `UnknownAccount` | `unknown_account` |
| `InvalidRequest` | `invalid_request` |
| `VerifierUnavailable` | `verifier_unavailable` |
| `InternalError` | `internal_error` |
| `ServiceUnavailable` | `service_unavailable` |

```rust
pub enum DenialCategory {
    InvalidCredentials,
    UnknownAccount,
    InvalidRequest,
    VerifierUnavailable,
    InternalError,
    ServiceUnavailable,
}
```

The type doc comment states: `"Opaque denial category returned on the
wire in nats.error."`

### 9.2 Error-to-category mapping

`DenialCategory::from_auth_callout_error` MUST map every
`AuthCalloutError` variant to exactly one category deterministically
(no variant maps to more than one category, and the match is
exhaustive over the error enum). `CredentialError` sub-variants map as:
`UnknownAccount -> UnknownAccount`, `VerifierUnavailable { .. } -> VerifierUnavailable`,
`InvalidRequest(_) -> InvalidRequest`, `InvalidCredentials(_) -> InvalidCredentials`.
All other `AuthCalloutError` variants (transport, subscribe, serialize,
JWT, wire-format, internal, key-loading) map to `InternalError` or
`ServiceUnavailable` as shown in the code quoted in Section 9's
evidence file.

### 9.3 Category is the only thing that reaches the wire

On a dispatch failure, the subscriber MUST log the full typed error
server-side via `tracing::warn` and MUST send only the `DenialCategory`
string (wrapped in a signed `DenialReason`, itself wrapped in a signed
denial JWT via `DenialClaims::mint`) to the client. No verification
detail, error message text, or configuration information from the
`AuthCalloutError` MUST appear in the wire response.

```rust
warn!(error = %e, "auth callout denied; sending opaque category reply");
let category = DenialCategory::from_auth_callout_error(&e);
let reason = DenialReason::new(category)...;
publish_denial(&client, &reply, &wire, &request, reason.as_str().to_owned()).await
```

### 9.4 Denial reason bounds

`DenialReason::new` / `DenialReason::from_wire` MUST reject an empty
string and MUST reject a string longer than 256 characters
(`MAX_LEN = 256`).

### 9.5 Denial JWT shape

The denial response MUST be a signed JWT (HS256, per
`DenialClaims::mint`) whose claims embed the category string as
`nats.error`, with `nats.type = "authorization_response"` and
`nats.version = 2`, alongside standard `iss`/`aud`/`sub`/`iat`/`exp`/`jti`
claims (`jti` is a UUIDv4). This matches the NATS server's expected
authorization-response error shape.

### 9.6 Fallback for construction failure

If `DenialReason::new` itself fails (only possible via the `TooLong`
path, since `DenialCategory::as_str()` values are all non-empty), the
subscriber MUST fall back to `DenialCategory::InternalError` rather
than propagating the construction error, and this fallback path is
documented as infallible (`.expect("internal_error reason is non-empty")`).

---

## 10. AAuth Token Verification

Evidence: `rsworkspace/crates/trogon-aauth-verify/src/token.rs`,
`rsworkspace/crates/trogon-identity-types/src/aauth/mod.rs`.

### 10.1 Token types

Three JWT `typ` header values MUST be recognized, each with a distinct
claim shape:

| `typ` constant | Value | Claims struct | Issued by |
| --- | --- | --- | --- |
| `TYP_AGENT` | `aa-agent+jwt` | `AgentClaims` | Agent Provider (bootstrap) |
| `TYP_RESOURCE` | `aa-resource+jwt` | `ResourceClaims` | Resource server (challenge) |
| `TYP_AUTH` | `aa-auth+jwt` | `AuthClaims` | Person Server / Authorization Server |

`parse_typ(jwt, expected)` MUST reject a JWT whose header `typ` does
not exactly equal the expected constant for the verification method
being called (`verify_agent` requires `TYP_AGENT`, etc.), returning
`TokenError::WrongTyp { expected, actual }`.

### 10.2 Algorithm allow-list

`parse_typ` MUST reject any JWT whose header `alg` is not one of
`ES256`, `ES384`, or `EdDSA`:

```rust
let alg = header.alg;
if !matches!(alg, Algorithm::ES256 | Algorithm::ES384 | Algorithm::EdDSA) {
    return Err(TokenError::UnsupportedAlg(alg));
}
```

RS256/RS384/RS512 (and every other `jsonwebtoken::Algorithm` variant,
including HS256) MUST be rejected by this check before any JWKS lookup
or signature verification is attempted. There is no dedicated
RSA-specific error variant or message; rejection is purely structural,
via `TokenError::UnsupportedAlg(alg)` on the `matches!` fallthrough.
This differs from the `a2a-auth-callout` OIDC path (Section 5.2), which
is RSA-only for a completely different JWT family (connection bearer
tokens, not AAuth tokens); the two allow-lists are disjoint and must
not be conflated.

### 10.3 JWK compatibility matching

`pick_jwk` MUST select a JWK from the resolved `JwkSet` only if it is
algorithm-compatible with the token's header `alg`, per
`jwk_compatible_with_alg`:

```rust
fn jwk_compatible_with_alg(jwk: &Jwk, alg: Algorithm) -> bool {
    match (&jwk.algorithm, alg) {
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES256) => ec.curve == EllipticCurve::P256,
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES384) => ec.curve == EllipticCurve::P384,
        (AlgorithmParameters::OctetKeyPair(okp), Algorithm::EdDSA) => okp.curve == EllipticCurve::Ed25519,
        _ => false,
    }
}
```

If a `kid` is present in the JWT header, the matching JWK MUST be
selected by `kid` among compatible keys. If no `kid` is present, a
match MUST only be made when exactly one compatible key exists in the
set (`compat.len() == 1`); otherwise verification MUST fail with
`TokenError::NoCompatibleJwk`.

### 10.4 Freshness

`assert_freshness(iat, exp)` MUST use the injected `TimeSource` clock
(never `SystemTime` directly) and MUST apply a leeway, defaulting to 60
seconds (`leeway_secs: 60` in `TokenVerifier::new`), configurable via
`with_leeway`. A token MUST be rejected as `NotYetValid` if
`now + leeway < iat`, and as `Expired` if `now - leeway > exp`.

`jsonwebtoken`'s own `exp`/`nbf` checks MUST be disabled
(`validation.validate_exp = false; validation.validate_nbf = false;`)
so that all freshness checks route through the single injected clock
rather than partially through `jsonwebtoken`'s internal `SystemTime`
usage.

### 10.5 Issuer-first JWKS resolution

The verifier MUST extract `iss` from the token's unverified payload
first (`iss_of(jwt)`), use it to resolve the applicable JWKS via the
injected `JwksResolver`, and only then validate the signature against
that resolved key set with `validation.set_issuer(&[iss.as_str()])`.

### 10.6 Audience validation

When an expected audience is supplied (`verify_resource`,
`verify_auth`), `validation.validate_aud = true` and
`validation.set_audience(&[aud])` MUST be set; `verify_agent` (no
audience parameter) MUST leave audience validation disabled.

---

## 11. NATS Proof of Possession

Evidence: `rsworkspace/crates/trogon-aauth-verify/src/nats_pop.rs`,
`rsworkspace/crates/trogon-identity-types/src/aauth/{mod.rs,headers.rs}`.

### 11.1 Conceptual model

`NatsPopVerifier::verify` mirrors RFC 9421 (HTTP message signatures)
conceptually, adapted for NATS messages which have no native request
line to sign over. The module doc states this explicitly: "Mirrors RFC
9421 conceptually but for NATS."

### 11.2 Required headers

Verification MUST require all of the following headers to be present;
a missing header MUST fail with `NatsPopError::MissingHeader(name)`:

| Constant | Wire header name |
| --- | --- |
| `headers::NATS_TOKEN` | `AAuth-Token` |
| `headers::NATS_SIG_INPUT` | `AAuth-Sig-Input` |
| `headers::NATS_SIG` | `AAuth-Sig` |
| `headers::NATS_SIG_CREATED` | `AAuth-Sig-Created` |
| `headers::NATS_SIG_NONCE` | `AAuth-Sig-Nonce` |
| `headers::CONTENT_DIGEST` | `Content-Digest` |

### 11.3 Duplicate security-header rejection

Every header in Section 11.2's table is a "security-sensitive header."
If any of them appears more than once, case-insensitively, in a
request's header set, verification MUST fail with
`NatsPopError::DuplicateHeader(name)`. This check MUST be performed in
two places, both unconditional:

1. `NatsHeaders::new_checked` (constructor-time), for callers that want
   to fail fast at header-view construction.
2. `NatsPopVerifier::verify` itself, via
   `req.headers.ensure_no_duplicate_security_headers()?` as the first
   check after the `max_skew_secs` guard, as defense-in-depth against a
   caller that built the header view with the unchecked `NatsHeaders::new`.

```rust
const SECURITY_HEADERS: &[&str] = &[
    headers::NATS_TOKEN, headers::NATS_SIG_INPUT, headers::NATS_SIG,
    headers::NATS_SIG_CREATED, headers::NATS_SIG_NONCE, headers::CONTENT_DIGEST,
];
```

Stated rationale in code: duplicate `AAuth-Sig` or `AAuth-Token`
headers are a header-smuggling vector where a signature check could
read one value while a downstream consumer reads another.

### 11.4 Clock skew

`max_skew_secs` MUST default to 300 seconds (`NatsPopVerifier::new`).
A negative configured value MUST be rejected at the start of `verify`
with `NatsPopError::NegativeMaxSkew`, before any header parsing.
Skew comparison MUST use checked/overflow-safe subtraction
(`now.checked_sub(created)`); on overflow, verification MUST fail with
`NatsPopError::SkewOverflow` rather than silently wrapping. A skew
whose absolute value exceeds `max_skew_secs` MUST fail with
`NatsPopError::Skew`.

### 11.5 Content-Digest requirement

The verifier MUST require an explicit `Content-Digest` header and MUST
NOT synthesize one from the payload if the header is absent. The
expected value MUST be computed as:

```rust
pub fn content_digest_sha256(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    format!("sha-256=:{}:", URL_SAFE_NO_PAD.encode(digest))
}
```

i.e. the literal prefix `sha-256=:`, the SHA-256 digest of the raw
payload bytes encoded as URL-safe base64 without padding, and a
trailing `:`. A supplied digest that does not match this computed
value MUST fail with `NatsPopError::DigestMismatch`.

Stated rationale: requiring rather than synthesizing the header closes
a gap where an attacker could omit `Content-Digest` and still pass
signature verification if the signing input didn't happen to commit to
the payload.

### 11.6 Canonical signature base

The canonical base string signed by the agent and reconstructed by the
verifier MUST be produced by `NatsSignatureEnvelope::canonical_base`:

```rust
pub fn canonical_base(&self, subject: &str, reply: Option<&str>, jkt: &str) -> String {
    let reply = reply.unwrap_or("");
    format!(
        concat!(
            "\"@subject\": {subject}\n",
            "\"@reply\": {reply}\n",
            "\"content-digest\": {digest}\n",
            "\"aauth-token\": {token}\n",
            "\"aauth-sig-created\": {created}\n",
            "\"aauth-sig-nonce\": {nonce}\n",
            "\"@signature-params\": {input};created={created};keyid=\"{kid}\""
        ),
        subject = subject, reply = reply, digest = self.content_digest,
        token = self.token, created = self.created, nonce = self.nonce,
        input = self.sig_input, kid = jkt,
    )
}
```

An absent `reply` subject MUST be rendered as an empty string, not
omitted from the base. The `keyid` component of `@signature-params`
MUST be the JWK thumbprint (`jkt`) of the agent's `cnf.jwk`, not an
arbitrary caller-supplied identifier.

### 11.7 Signature verification against `cnf.jwk`

The signature MUST be verified against the `cnf.jwk` embedded in the
already-verified `aa-agent+jwt` (Section 10), using only ES256, ES384,
or EdDSA, selected from the JWK's own `kty`/`crv`:

```rust
let alg = match &jwk.algorithm {
    AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P256 => Algorithm::ES256,
    AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P384 => Algorithm::ES384,
    AlgorithmParameters::OctetKeyPair(okp) if okp.curve == EllipticCurve::Ed25519 => Algorithm::EdDSA,
    _ => return Err(NatsPopError::InvalidConfirmationKey(InvalidConfirmationKey::UnsupportedAlgorithm)),
};
```

A `cnf.jwk` naming any other key type or curve MUST be rejected with
`InvalidConfirmationKey::UnsupportedAlgorithm` before any signature
verification is attempted. The signature MUST be verified over the raw
canonical base bytes (no JWS re-wrapping), with the signature itself
supplied as base64url-no-pad.

### 11.8 Ordering: signature before nonce consumption

The replay-protection nonce MUST NOT be consumed
(`ReplayStore::check_and_insert`) until *after* the PoP signature has
been verified successfully. Evidence and stated rationale in
`nats_pop.rs`:

```rust
// Verify the signature BEFORE consuming the nonce. Otherwise an
// invalid-signature request burns its nonce against the replay store
// and a later valid retry from the same agent is wrongly rejected as
// a replay.
let canonical = envelope.canonical_base(req.subject, req.reply, &verified_agent.jkt);
verify_signature_with_jwk(&verified_agent.claims.cnf.jwk, canonical.as_bytes(), sig)?;
```

---

## 12. Replay Protection

Evidence: `rsworkspace/crates/trogon-aauth-verify/src/replay.rs`,
`nats_pop.rs`.

### 12.1 `ReplayStore` contract

The `ReplayStore` trait MUST expose exactly one operation:

```rust
async fn check_and_insert(&self, key: &str, ttl_secs: u32) -> Result<bool, ReplayError>;
```

returning `Ok(true)` if and only if `key` was newly inserted (i.e. not
previously seen and still live), and `Ok(false)` if `key` was already
present. Backend failures MUST surface as a typed `ReplayError`
(`MutexPoisoned` for the in-memory store, `Backend(source)` for
pluggable backends), never silently treated as "fresh."

### 12.2 Nonce key derivation and TTL

The PoP verifier MUST derive the replay-store key as
`format!("nats-pop:{nonce}")` where `nonce` is the value of the
`AAuth-Sig-Nonce` header, and MUST compute the TTL as:

```rust
const MIN_REPLAY_TTL_SECS: i64 = 60;
let ttl_secs = self.max_skew_secs.saturating_mul(2).max(MIN_REPLAY_TTL_SECS);
```

i.e. `max(max_skew_secs * 2, 60)`. This floor exists so that a
`max_skew_secs = 0` deployment does not install a zero-second-TTL
nonce record that would be garbage-collected before it could ever
reject a genuine replay.

### 12.3 Replay outcome

If `check_and_insert` returns `Ok(false)` (nonce already seen and
still live), verification MUST fail with `NatsPopError::Replay`.

### 12.4 In-memory implementation is the only bundled production path

`InMemoryReplayStore` MUST use a `Mutex<HashMap<String, i64>>` keyed by
nonce with expiry timestamps, garbage-collecting expired entries
(`gc`) on every `check_and_insert` call before checking or inserting
the current key. Its doc comment states this is "best-effort... suitable
for a single-process gateway or unit tests," and that "Multi-instance
deployments should use the JetStream-backed store" -- which this crate
does not implement (see Section 15).

---

## 13. Failure Semantics

| Layer | Failure condition | Behavior |
| --- | --- | --- |
| Auth callout: request decode | Malformed/undecodable wire envelope | Publish empty reply payload; NATS server denies connect immediately (`subscriber.rs`) |
| Auth callout: dispatch | Any `AuthCalloutError` | Log full typed error via `tracing::warn` server-side; publish a signed denial JWT carrying only the opaque `DenialCategory` string (Section 9) |
| Auth callout: reply publish | Publish itself fails | Fall back to publishing an empty payload |
| Auth callout: encryption config | Only one of `account_xkey_seed` / `server_xkey_public` set | Reject at construction |
| OIDC verify | Discovery issuer mismatch, cross-origin `jwks_uri`, non-RSA JWK, missing `kid`, no matching JWK, bad signature, `iss`/`aud` mismatch | `CredentialError::InvalidCredentials`, mapped to `DenialCategory::InvalidCredentials` |
| mTLS verify | Expired/not-yet-valid cert, CA presented as leaf, chain does not reach a trust anchor | `CredentialError::InvalidCredentials` |
| AAuth token verify | Wrong `typ`, disallowed `alg`, no compatible JWK, expired/not-yet-valid, audience mismatch | Typed `TokenError` variant (Section 10) |
| NATS PoP verify | Missing/duplicate header, negative or overflowing skew check, skew exceeded, digest mismatch, bad signature, unsupported `cnf.jwk` algorithm, nonce replay, replay-backend failure | Typed `NatsPopError` variant (Section 11); no fallback to a permissive default in any branch |
| Replay store | Mutex poisoned (in-memory) | `ReplayError::MutexPoisoned`; caller MUST treat as failed request, not as "fresh" |

No code path reviewed in either crate falls back to an "allow" or
"permissive default" outcome on verification failure; every failure
listed above produces a distinct error value that a caller must
explicitly map to a denial.

---

## 14. Security Considerations

### 14.1 Wire-visible denial detail is intentionally minimal

`DenialCategory` is deliberately a closed six-variant enum with no
free-text payload (Section 9). Operators depend on server-side
`tracing::warn` logs, not the wire response, to diagnose specific auth
failures. There is no wire-visible correlation ID beyond the request's
own NATS reply subject in the code reviewed.

### 14.2 Header smuggling

The duplicate-header check (Section 11.3) exists specifically to
prevent a class of attack where a signature-verification code path and
a downstream consumer disagree on which of several same-named header
values is authoritative. It is enforced twice (constructor and
verifier) so that no call site can accidentally bypass it.

### 14.3 Digest binding

Requiring rather than synthesizing `Content-Digest` (Section 11.5)
ensures the signed canonical base always commits to the actual payload
bytes that will be processed, rather than to a value the verifier
computed independently of what the signature covers.

### 14.4 Nonce/signature ordering

Verifying the PoP signature before consuming the replay nonce (Section
11.8) prevents an attacker from using invalid-signature requests to
"poison" a legitimate agent's nonce namespace and cause its own valid
retries to be rejected as replays.

### 14.5 OIDC discovery origin pinning

Same-origin enforcement between the configured issuer and the
discovered `jwks_uri` (Section 5.1) exists specifically to stop a
tampered discovery document from redirecting JWKS fetches to an
attacker-controlled host, even when the `issuer` field in the document
matches.

### 14.6 Redacted token formatting

`CallerJwtHeaderValue`'s `Display` impl always renders `"<redacted>"`
(Section 7.5). This protects against accidental leakage through
`tracing`/`{}` formatting call sites, not against a developer who
explicitly calls `.as_str()` and logs the result.

### 14.7 Subject-pattern re-validation on deserialize

`SubjectPattern`'s hand-written `Deserialize` (Section 8.1) exists so
that ACL permissions embedded inside a previously-minted JWT are
re-validated on decode, rather than trusted as opaque strings that
skipped the constructor's whitespace/emptiness checks.

---

## 15. Non-Guarantees

The following are explicitly **not** guaranteed by the code reviewed,
despite being easy to assume from adjacent behavior:

1. **No CRL/OCSP revocation checking.** The mTLS chain walk (Section
   6.4) checks validity windows and CA/end-entity constraints, but
   performs no revocation check against any CRL or OCSP responder. A
   compromised but not-yet-expired client certificate would still
   verify.
2. **No cross-instance replay protection by default.** The only
   bundled `ReplayStore` implementation, `InMemoryReplayStore`, is
   process-local. A multi-replica deployment that does not supply an
   external `ReplayStore` (e.g. a JetStream-KV-backed one, referenced
   in code comments as living in `trogon-aauth-person`, not in this
   crate) has no replay protection across replicas -- a nonce accepted
   by one instance is unknown to the others.
3. **OIDC signature verification is RSA-only.** An identity provider
   that only publishes EC or OKP (Ed25519) signing keys in its OIDC
   JWKS will fail every token verification on the `a2a-auth-callout`
   OIDC path (Section 5.2). This is a current, real limitation, not a
   theoretical one.
4. **No wire-visible correlation ID for denials.** Beyond the NATS
   reply subject, nothing in the denial response links a specific
   denied connection attempt to a specific server-side log line; that
   correlation must be done out of band (e.g. by timestamp and reply
   subject) if needed.
5. **AAuth verification is server-side only.** `trogon-aauth-verify`
   does not sign requests; the agent-side signer is documented as
   living in a separate `trogon-aauth-sdk` crate, which is out of
   scope for this specification and was not reviewed.
6. **The `Cnf`/JWK payload is not schema-validated beyond
   algorithm/curve compatibility.** `cnf.jwk` is stored as an untyped
   `serde_json::Value` in `AgentClaims` and only inspected for
   algorithm/curve fields during PoP verification; no broader JWK
   schema validation is performed by these crates.
7. **`ensure_fresh()` on a minted user JWT is not a security check.**
   `MintedUserJwt::ensure_fresh()` decodes the payload without
   verifying the signature. It is a client-side convenience for
   deciding whether to request a new token before sending a request; it
   MUST NOT be relied upon as an authentication or authorization
   decision point.
8. **Auth-callout encryption is optional, not mandatory.** The
   both-or-neither Xkey configuration invariant (Section 3.3) only
   guarantees consistency when encryption *is* configured; it does not
   require operators to configure Xkey encryption at all.

---

## 16. Conformance Checklist

An implementation claiming conformance with the behavior described in
this document exhibits all of the following, each traceable to the
cited evidence:

- [ ] Auth-callout server subscribes via `queue_subscribe` on
      `$SYS.REQ.USER.AUTH` with a configurable, defaulted queue group
      (Section 3.1).
- [ ] Undecodable requests and reply-publish failures both fall back to
      an empty-payload reply rather than leaving the client to time out
      (Section 3.2).
- [ ] Credential scheme selection follows the fixed OIDC -> mTLS ->
      API-key preference order and never combines schemes (Section 4.1).
- [ ] OIDC discovery enforces a bounded timeout, disables redirects,
      and checks both issuer match and `jwks_uri` same-origin
      (Section 5.1).
- [ ] OIDC JWK decoding accepts only RSA algorithm parameters
      (Section 5.2).
- [ ] mTLS verification checks certificate validity window,
      RFC 5280 `basicConstraints.cA`, and requires the chain to
      terminate at a configured trust anchor (Section 6.2-6.4).
- [ ] Minted user JWTs use header `alg: "ed25519-nkey"` and compute
      `jti` as `Sha512_256` over the claim body with `iss` populated
      and `jti` empty at hash time (Section 7.1-7.2).
- [ ] `IssuedPermissions::default_for_caller` grants exactly
      `a2a.gateway.>` publish and `_INBOX.{caller}.>` /
      `a2a.push.{caller}.>` subscribe, no more (Section 8.2).
- [ ] `DenialCategory` is a closed six-variant enum and is the only
      failure detail that reaches the wire (Section 9.1, 9.3).
- [ ] AAuth token verification rejects any `alg` outside
      `{ES256, ES384, EdDSA}` before JWKS lookup (Section 10.2).
- [ ] AAuth JWK selection requires algorithm/curve compatibility and,
      absent a `kid`, requires exactly one compatible candidate
      (Section 10.3).
- [ ] AAuth freshness checks route entirely through the injected
      `TimeSource`, with `jsonwebtoken`'s own `exp`/`nbf` checks
      disabled (Section 10.4).
- [ ] NATS PoP verification rejects duplicate security-sensitive
      headers case-insensitively, checked in two places (Section 11.3).
- [ ] NATS PoP verification requires an explicit `Content-Digest`
      header in `sha-256=:<url-safe-base64-no-pad>:` form and never
      synthesizes one (Section 11.5).
- [ ] The canonical signature base is built by
      `NatsSignatureEnvelope::canonical_base` over exactly
      `@subject`, `@reply`, `content-digest`, `aauth-token`,
      `aauth-sig-created`, `aauth-sig-nonce`, and `@signature-params`
      (Section 11.6).
- [ ] PoP signature verification happens strictly before replay-nonce
      consumption (Section 11.8).
- [ ] Replay TTL is `max(max_skew_secs * 2, 60)` seconds
      (Section 12.2).
- [ ] No verification failure path in either crate falls back to an
      allow/permissive outcome (Section 13).

---

## 17. Worked Examples

### 17.1 AAuth resource-challenge JWT (ES384), from a real unit test

`rsworkspace/crates/trogon-aauth-verify/src/token/tests.rs`,
`verify_resource_accepts_es384_jwk`, constructs and verifies this
exact claim set with header `alg: ES384`, `typ: "aa-resource+jwt"`,
`kid: "p384-k1"`:

```json
{
  "iss": "iss.example",
  "aud": "ps.example",
  "jti": "j1",
  "iat": 1000,
  "exp": 9999999999,
  "dwk": "aa-resource",
  "agent": "agent-1",
  "agent_jkt": "abc",
  "scope": "read"
}
```

verified against a JWKS containing exactly this ES384 JWK
(`test_support::p384_fixture`, `P384_JWK_JSON`):

```json
{
  "kty": "EC",
  "crv": "P-384",
  "kid": "p384-k1",
  "alg": "ES384",
  "x": "vt3D7Exqb0PMRX8qp01x0xeXtOwUqcJyu7dhnRfisg8q_U05rG_SWx6BGqhPpN3A",
  "y": "fymA6wvqJ0A7KcQ_hGHG3Ki7lFfcr_-XxGloWpdvCCztVBCoZpdcejX1xDY4a9PV"
}
```

`TokenVerifier::verify_resource(&jwt, "ps.example")` MUST succeed for
this pair (this is the exact assertion in the cited test).

### 17.2 NATS PoP canonical base, reconstructed from `nats_pop.rs`

Given a request with:

- `subject = "a2a.gateway.invoke"`, `reply = None`
- `payload = b"{...request body...}"`
- headers: `AAuth-Token: <aa-agent+jwt>`, `AAuth-Sig-Input: sig1`,
  `AAuth-Sig-Created: 1750000000`, `AAuth-Sig-Nonce: n-abc123`,
  `Content-Digest: sha-256:<computed>`

`content_digest_sha256(payload)` MUST first be computed as:

```
sha-256=:<URL_SAFE_NO_PAD base64 of SHA-256(payload)>:
```

and MUST equal the supplied `Content-Digest` header value exactly, or
verification fails with `DigestMismatch` before any signature check.

Given the agent's `cnf.jwk` has JWK thumbprint `jkt = "abc123thumb"`,
the canonical base string that must have been signed is exactly:

```
"@subject": a2a.gateway.invoke
"@reply": 
"content-digest": sha-256=:<digest>:
"aauth-token": <aa-agent+jwt>
"aauth-sig-created": 1750000000
"aauth-sig-nonce": n-abc123
"@signature-params": sig1;created=1750000000;keyid="abc123thumb"
```

Note the empty `"@reply": ` line (a trailing space, empty value, not
an omitted line) when no reply subject is present -- this is exactly
what `canonical_base`'s `format!` macro produces for `reply.unwrap_or("")`.

### 17.3 Duplicate-header rejection, from a real unit test

`rsworkspace/crates/trogon-aauth-verify/src/nats_pop/tests.rs`,
`verify_rejects_duplicate_security_headers_even_with_unchecked_new`,
constructs a request with two `AAuth-Token` header entries:

```rust
let items = vec![
    (headers::NATS_TOKEN.to_string(), "first".into()),
    (headers::NATS_TOKEN.to_string(), "second".into()),
];
```

and asserts `verifier.verify(&req).await` returns
`NatsPopError::DuplicateHeader(name) if name == headers::NATS_TOKEN`,
even though the headers were constructed via the unchecked
`NatsHeaders::new` (not `new_checked`) -- demonstrating the
defense-in-depth duplicate check inside `verify` itself (Section 11.3).

### 17.4 Denial response shape

For a dispatch failure mapped to `DenialCategory::InvalidCredentials`,
the signed denial JWT's claims (per `DenialClaims::mint`) take this
shape:

```json
{
  "iss": "<configured callout issuer>",
  "aud": "<server audience>",
  "sub": "<user nkey subject>",
  "iat": 1700000000,
  "exp": 1700000060,
  "jti": "<uuid-v4>",
  "nats": {
    "error": "invalid_credentials",
    "type": "authorization_response",
    "version": 2
  }
}
```

`nats.error` MUST be one of the six wire strings from Section 9.1 and
nothing else; there is no field anywhere in this shape that carries
free-text diagnostic detail.

### 17.5 Minted NATS user-permission ACL for a caller

For `caller_id = "caller-42"`, `IssuedPermissions::default_for_caller`
produces, and `mint_nats_user_jwt` embeds into the `nats.pub`/`nats.sub`
blocks of the signed user JWT:

```json
{
  "pub": { "allow": ["a2a.gateway.>"] },
  "sub": { "allow": ["_INBOX.caller-42.>", "a2a.push.caller-42.>"] }
}
```

---

## 18. References

- [RFC 2119: Key words for use in RFCs](https://datatracker.ietf.org/doc/html/rfc2119)
- [RFC 8174: Ambiguity of Uppercase vs Lowercase in RFC 2119](https://datatracker.ietf.org/doc/html/rfc8174)
- [RFC 9421: HTTP Message Signatures](https://datatracker.ietf.org/doc/html/rfc9421)
- [RFC 5280: Internet X.509 Public Key Infrastructure Certificate and CRL Profile](https://datatracker.ietf.org/doc/html/rfc5280)
- [RFC 7517: JSON Web Key (JWK)](https://datatracker.ietf.org/doc/html/rfc7517)
- `draft-hardt-aauth-protocol` (AAuth), as implemented in
  `trogon-identity-types::aauth`
- `rsworkspace/crates/a2a-auth-callout/` (source of truth for Sections 3-9)
- `rsworkspace/crates/trogon-aauth-verify/` (source of truth for Sections 10-12)
- `rsworkspace/crates/a2a-identity-types/` (wire identity types shared by the callout path)
- `rsworkspace/crates/trogon-identity-types/` (AAuth wire types shared beyond A2A)
- `.trogonai/analysis/agt-vs-trogonai/survey-identity-auth.internal.trogonai.md` (background survey)
- Microsoft Agent Governance Toolkit, `docs/specs/AGENTMESH-IDENTITY-TRUST-1.0.md` (MIT License; document structure reference only)
