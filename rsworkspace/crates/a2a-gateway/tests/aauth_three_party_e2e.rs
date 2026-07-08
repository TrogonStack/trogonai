//! Capstone end-to-end test for the draft's three-party "PS-Asserted Access"
//! mode (draft-hardt-oauth-aauth-protocol), exercised entirely over the
//! gateway's NATS binding plus a real Person Server HTTP exchange.
//!
//! Cast of parties:
//! - **Agent Provider (AP)**: mints the agent's `aa-agent+jwt` (identity +
//!   `cnf.jwk` proof-of-possession key). Uses `trogon_jwks_publisher::provider`.
//! - **Resource (the gateway)**: `a2a_gateway::aauth::AAuthIngress` in
//!   enforce mode. Verifies inbound NATS PoP envelopes, and mints
//!   `aa-resource+jwt` challenges when access is refused.
//! - **Person Server (PS)**: `trogon_aauth_person::PersonServer`, served over
//!   real HTTP (axum + a loopback `TcpListener`). Applies "the person's
//!   policy" to a token request and mints `aa-auth+jwt` grants.
//! - **Agent**: a single P-256 key driving both the NATS PoP signing
//!   (`trogon_aauth_sdk::AgentSigner`) and the PS token exchange
//!   (`trogon_aauth_sdk::exchange::PsTokenClient`).
//!
//! Protocol walk (draft section references in each step below):
//!
//! 1. Issuance ("Obtaining an Agent Token") -- AP mints `aa-agent+jwt`.
//! 2. Resource challenge ("Resource Challenge") -- the agent calls the
//!    gateway with no auth token; enforce mode denies with code -32118 and a
//!    `aa-resource+jwt` challenge bound to the agent.
//! 3. PS exchange ("Agent Token Request" / "PS Response") -- the agent
//!    presents its agent token + the resource challenge to the PS's
//!    `/token` endpoint; policy grants immediately, minting `aa-auth+jwt`.
//! 4. Response verification ("Auth Token -- Structure, Usage, Verification"
//!    rules 1-6) -- the agent verifies the grant against its own key/id.
//! 5. Authorized presentation ("Scopes") -- the agent re-signs a NATS
//!    request carrying the auth token; the gateway resolves it to the PS's
//!    asserted access, scoped to a method the granted scope covers.
//! 6. Negative coda ("Scopes") -- the same auth token against a method
//!    outside its granted scope denies with `ScopeNotCovered`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_arguments, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;

use a2a_gateway::aauth::{
    AAUTH_REQUIRED_CODE, AAuthConfig, AAuthDenyReason, AAuthIngress, AAuthMode, ChallengeKid, LeewaySecs,
    NonNegativeSecs, PersonServerAudience, ResourceIssuer, StaticJwks,
};
use trogon_aauth_person::PersonServer;
use trogon_aauth_person::decision::{DecisionRequest, PolicyDecision, PolicyEngine};
use trogon_aauth_person::interaction::NoopInteractionChannel;
use trogon_aauth_person::store::InMemoryStore;
use trogon_aauth_sdk::exchange::{ExchangeOutcome, PsTokenClient, SignatureKeyOnlyHttpSigner, build_token_request};
use trogon_aauth_sdk::verify_response::verify_auth_claims;
use trogon_aauth_sdk::{AgentSigner, jwk_thumbprint};
use trogon_aauth_verify::{InMemoryReplayStore, SystemTimeSource};
use trogon_identity_types::aauth::TYP_AUTH;
use trogon_identity_types::aauth::person_server::TokenRequest;

use trogon_jwks_publisher::provider::{
    AgentIdentifier, AgentProvider, AgentProviderKey, AgentTokenRequest, KeyId, PersonServerUrl, ProviderIssuer,
    TokenTtl,
};

/// The scope the PS's fixture policy grants. Deliberately a method-prefix
/// glob, not `"*"`, so step 6 has a real "method outside scope" to probe --
/// see `a2a_gateway::aauth::scope_covers_method`'s grammar.
const GRANTED_SCOPE: &str = "message.*";
/// A method the granted scope does NOT cover (draft "Scopes"): used only in
/// the negative coda.
const UNCOVERED_METHOD: &str = "tasks.get";
/// A method the granted scope DOES cover: used for the authorized NATS call.
const COVERED_METHOD: &str = "message.send";

fn pkcs8_pem(sk: &SigningKey) -> String {
    sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).expect("pkcs8").to_string()
}

/// Mints a standalone `aa-auth+jwt` scoped to `scope`, signed by the AP as a
/// stand-in "the agent already holds some auth token, just not a broad
/// enough one" fixture (mirrors `aauth_roundtrip.rs`'s `mint_auth_jwt_with`
/// idiom). Not part of the real PS exchange -- that happens for real via
/// `trogon_aauth_person::PersonServer` in step 3 -- this is only used to
/// drive the `ScopeNotCovered` deny path in step 2's composability note.
fn mint_scope_limited_auth_jwt(
    ap_signing: &SigningKey,
    ap_kid: &str,
    ap_iss: &str,
    resource_iss: &str,
    agent_sub: &str,
    agent_jkt: &str,
    scope: &str,
) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);
    let enc = EncodingKey::from_ec_pem(pkcs8_pem(ap_signing).as_bytes()).expect("enc");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some(ap_kid.into());
    let claims = serde_json::json!({
        "iss": ap_iss,
        "sub": agent_sub,
        "aud": resource_iss,
        "jti": format!("insufficient-scope-jti-{agent_sub}"),
        "iat": now - 5,
        "exp": now + 600,
        "agent": agent_sub,
        "agent_jkt": agent_jkt,
        "scope": scope,
    });
    encode(&header, &claims, &enc).expect("encode auth jwt")
}

/// Builds a `jsonwebtoken::jwk::Jwk` (the JWKS-wire shape) for a signing
/// key's public half, keyed by `kid`. Every party in this test (AP, gateway's
/// challenge signer is skipped -- self-signed and unverified by the test --
/// and the PS) publishes its public key this way so `StaticJwks` can resolve
/// it by issuer.
fn public_jwk_entry(signing_key: &SigningKey, kid: &str) -> Jwk {
    let point = signing_key.verifying_key().to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
    Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(kid.to_string()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x,
            y,
        }),
    }
}

/// Fixture policy engine for the PS: grants `GRANTED_SCOPE` on the first
/// evaluation of every request, mirroring `trogon-aauth-person`'s own
/// `person_server_e2e.rs` `ScriptedPolicy` idiom but without the scripted
/// queue -- this test only ever needs one grant.
struct GrantImmediately;

#[derive(Debug, thiserror::Error)]
#[error("GrantImmediately never fails")]
struct GrantImmediatelyError;

#[async_trait::async_trait]
impl PolicyEngine for GrantImmediately {
    type Error = GrantImmediatelyError;

    async fn decide(&self, _request: DecisionRequest<'_>) -> Result<PolicyDecision, Self::Error> {
        Ok(PolicyDecision::Grant {
            scope: GRANTED_SCOPE.to_string(),
        })
    }
}

type TestPersonServer =
    PersonServer<StaticJwks, SystemTimeSource, GrantImmediately, NoopInteractionChannel, InMemoryStore>;

#[tokio::test(flavor = "multi_thread")]
async fn three_party_ps_asserted_access_over_nats() {
    // ------------------------------------------------------------------
    // Fixture setup: three independent signing identities (AP, gateway's
    // challenge key, PS) plus the agent's own P-256 key. Nothing here is
    // shared key material -- each party in the draft's three-party mode has
    // its own trust root, and the JWKS wiring below is what lets each
    // verifier resolve the *other* parties' public keys by issuer.
    // ------------------------------------------------------------------
    let ap_signing = SigningKey::random(&mut OsRng);
    let ap_kid = "ap-key-1";
    let ap_iss = "https://ap.test";

    let gateway_challenge_signing = SigningKey::random(&mut OsRng);
    let resource_iss = "https://resource.test";

    let ps_signing = SigningKey::random(&mut OsRng);
    let ps_kid = "ps-key-1";
    // The PS's own issuer URL. Per "PS Response" / `verify_request` (see
    // trogon-aauth-person/src/agent.rs), the resource token's `aud` must
    // equal this exact string -- so the gateway's `person_server_aud` config
    // (used as the challenge's `aud_ps`) must match it verbatim.
    let ps_iss = "https://ps.test";

    let agent_signing = SigningKey::random(&mut OsRng);

    // ------------------------------------------------------------------
    // Step 1 -- Issuance ("Obtaining an Agent Token"): the Agent Provider
    // mints a real `aa-agent+jwt` binding the agent's identifier to its
    // public confirmation key (`cnf.jwk`), using the typed provider API
    // instead of hand-rolled `jsonwebtoken::encode`.
    // ------------------------------------------------------------------
    let agent_jwk = {
        let point = agent_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y})
    };
    let agent_jkt_expected = jwk_thumbprint(&agent_jwk).expect("agent jkt");

    let provider_key = AgentProviderKey::new(
        EncodingKey::from_ec_pem(pkcs8_pem(&ap_signing).as_bytes()).expect("ap encoding key"),
        KeyId::new(ap_kid).expect("valid kid"),
        ProviderIssuer::new(ap_iss).expect("valid iss"),
    );
    let provider = AgentProvider::new(provider_key);
    let agent_id = AgentIdentifier::new("aauth:capstone-agent@agent.example").expect("valid agent identifier");
    let agent_jwt = provider
        .mint(AgentTokenRequest {
            sub: agent_id.clone(),
            agent_jwk: agent_jwk.clone(),
            ttl: TokenTtl::new(3600).expect("positive ttl"),
            ps: Some(PersonServerUrl::new(ps_iss).expect("valid ps url")),
        })
        .expect("mint aa-agent+jwt");

    // ------------------------------------------------------------------
    // Step 2 -- Resource challenge ("Resource Challenge"): build an
    // enforce-mode gateway ingress whose StaticJwks knows both the AP (to
    // verify the agent's PoP envelope) and the PS (wired below, once we know
    // its public key, so the gateway can later verify auth tokens the PS
    // mints). The agent signs a NATS request with NO auth token; enforce
    // mode must deny with -32118 and a resource challenge bound to this
    // agent.
    // ------------------------------------------------------------------
    let mut gateway_jwks = StaticJwks::new().with(
        ap_iss.to_string(),
        JwkSet {
            keys: vec![public_jwk_entry(&ap_signing, ap_kid)],
        },
    );
    gateway_jwks.insert(
        ps_iss.to_string(),
        JwkSet {
            keys: vec![public_jwk_entry(&ps_signing, ps_kid)],
        },
    );

    let challenge_key =
        EncodingKey::from_ec_pem(pkcs8_pem(&gateway_challenge_signing).as_bytes()).expect("challenge enc key");
    let ingress = Arc::new(AAuthIngress::new_in_memory(AAuthConfig {
        mode: AAuthMode::Enforce,
        jwks: gateway_jwks,
        resource_iss: ResourceIssuer::new(resource_iss).expect("resource iss"),
        // The challenge's `aud` must be the exact PS issuer the PS expects
        // as the resource token's `aud` (checked in
        // trogon-aauth-person/src/agent.rs `verify_request`).
        person_server_aud: PersonServerAudience::new(ps_iss).expect("ps aud"),
        leeway_secs: LeewaySecs::new(30),
        challenge_alg: Algorithm::ES256,
        challenge_key,
        challenge_kid: ChallengeKid::new("gw-challenge-kid").expect("kid"),
        challenge_ttl_secs: NonNegativeSecs::new(60).expect("ttl"),
        max_skew_secs: NonNegativeSecs::new(60).expect("skew"),
    }));

    // AgentSigner drives both the NATS PoP envelope now and (after the PS
    // grant) the authorized presentation in step 5 -- one signer, two
    // requests, `with_auth_token` swapped in between.
    let unauthenticated_signer = AgentSigner::new(agent_signing.clone(), agent_jwt.clone()).expect("agent signer");
    assert_eq!(
        unauthenticated_signer.jkt(),
        agent_jkt_expected,
        "signer's own jkt must match the jkt embedded in the minted agent token"
    );

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.capstone.1";
    let payload = br#"{"hello":"world"}"#.to_vec();
    let pop_headers = unauthenticated_signer
        .sign_nats_request_now(subject, Some(reply), &payload)
        .into_pairs();

    // COMPOSABILITY NOTE (see this test's final report for the full
    // write-up): the draft's "Resource Access and Resource Tokens" expects a
    // resource's FIRST reply to an agent with no auth token at all to be a
    // 401/402 carrying a fresh challenge (see
    // trogon-aauth-sdk/src/exchange/challenge.rs module docs, which document
    // the agent-side handling of exactly that reply). But
    // `AAuthIngress::resolve_nats` (crates/a2a-gateway/src/aauth.rs) does
    // NOT implement that: when PoP verification succeeds and `auth_token` is
    // `None`, it returns `Ok(AAuthResolution)` immediately -- see the
    // `AAuthResolution::scope` doc comment, "scope enforcement does not
    // apply to agent-only access". A well-formed PoP-only request is valid,
    // scope-unconstrained "agent-only access" in this binding, not a
    // -32118 deny. We assert that real behavior here first, then obtain a
    // genuine -32118 + challenge the way this binding actually produces one:
    // an agent that already holds an (insufficiently scoped) auth token
    // hits `ScopeNotCovered`, which mints the same
    // `requirement=auth-token` challenge a first-contact reply would carry.
    let agent_only = ingress
        .resolve_nats(subject, Some(reply), &payload, &pop_headers, None, COVERED_METHOD)
        .await
        .expect("PoP-verified agent with no auth token is valid agent-only access in this binding");
    assert_eq!(agent_only.agent_id.as_deref(), Some(agent_id.as_str()));
    assert!(
        agent_only.scope.is_none(),
        "agent-only access carries no scope -- there is no auth token to read one from"
    );

    // Mint a throwaway auth token scoped to nothing this agent is about to
    // call (`tasks.*`), signed by the AP purely as a stand-in "already have
    // some token, but not a broad enough one" fixture -- not part of the
    // real PS exchange, which happens for real in step 3 below.
    let insufficient_scope_auth_jwt = mint_scope_limited_auth_jwt(
        &ap_signing,
        ap_kid,
        ap_iss,
        resource_iss,
        agent_id.as_str(),
        &agent_jkt_expected,
        "tasks.*",
    );
    let insufficiently_scoped_signer = AgentSigner::new(agent_signing.clone(), agent_jwt.clone())
        .expect("agent signer")
        .with_auth_token(insufficient_scope_auth_jwt.clone());
    let insufficient_scope_headers = insufficiently_scoped_signer
        .sign_nats_request_now(subject, Some(reply), &payload)
        .into_pairs();
    let deny = ingress
        .resolve_nats(
            subject,
            Some(reply),
            &payload,
            &insufficient_scope_headers,
            Some(&insufficient_scope_auth_jwt),
            COVERED_METHOD,
        )
        .await
        .expect_err("scope tasks.* does not cover message.send -- enforce mode must deny");
    assert_eq!(
        deny.code, AAUTH_REQUIRED_CODE,
        "deny must carry the AAuth required code"
    );
    assert!(
        matches!(deny.reason, AAuthDenyReason::ScopeNotCovered { .. }),
        "expected ScopeNotCovered, got {:?}",
        deny.reason
    );
    let challenge = deny.challenge.clone().expect("deny must carry a resource challenge");

    // The challenge is an `aa-resource+jwt` bound to this agent. Decode its
    // payload (unverified -- the gateway self-signs with a throwaway test
    // key we didn't publish anywhere) purely to confirm the binding claims
    // the draft requires ("Resource Challenge" `agent` / `agent_jkt`).
    let challenge_claims = decode_jwt_payload_unverified(&challenge);
    assert_eq!(challenge_claims["agent"], serde_json::json!(agent_id.as_str()));
    assert_eq!(challenge_claims["agent_jkt"], serde_json::json!(agent_jkt_expected));
    assert_eq!(challenge_claims["iss"], serde_json::json!(resource_iss));
    assert_eq!(challenge_claims["aud"], serde_json::json!(ps_iss));

    // ------------------------------------------------------------------
    // Step 3 -- PS exchange ("Agent Token Request" / "PS Response"): stand
    // up the real Person Server axum Router on a loopback TCP listener, then
    // drive the exchange with the SDK's PsTokenClient presenting the agent
    // token + the resource challenge minted above.
    // ------------------------------------------------------------------
    let ps_jwks = StaticJwks::new()
        .with(
            ap_iss.to_string(),
            JwkSet {
                keys: vec![public_jwk_entry(&ap_signing, ap_kid)],
            },
        )
        .with(
            resource_iss.to_string(),
            JwkSet {
                keys: vec![public_jwk_entry(&gateway_challenge_signing, "gw-challenge-kid")],
            },
        );
    let ps_encoding_key = EncodingKey::from_ec_pem(pkcs8_pem(&ps_signing).as_bytes()).expect("ps enc key");
    let person_server: TestPersonServer = PersonServer::new(
        trogon_aauth_verify::TokenVerifier::new(ps_jwks, SystemTimeSource),
        SystemTimeSource,
        GrantImmediately,
        NoopInteractionChannel,
        InMemoryStore::new(),
        ps_encoding_key,
        Algorithm::ES256,
        ps_kid,
        ps_iss,
    );
    assert_eq!(
        person_server.iss(),
        ps_iss,
        "PS's own iss must equal the gateway's person_server_aud config"
    );

    let router = trogon_aauth_person::http::router(Arc::new(person_server));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let ps_addr: SocketAddr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("axum serve");
    });

    let token_endpoint = format!("http://{ps_addr}/token");
    let http_client = reqwest::Client::new();
    let http_signer = SignatureKeyOnlyHttpSigner::new(agent_jwt.clone());
    let ps_client = PsTokenClient::new(http_client, http_signer, token_endpoint);

    let token_request: TokenRequest =
        build_token_request(&agent_jwt, challenge.clone(), None).expect("build token request");
    let outcome = ps_client.request_token(&token_request).await;
    let grant = match outcome {
        ExchangeOutcome::Granted(grant) => grant,
        other => panic!("expected an immediate grant from the always-grant policy, got {other:?}"),
    };

    // ------------------------------------------------------------------
    // Step 4 -- Response verification ("Auth Token -- Structure, Usage,
    // Verification" rules 1-6): the agent verifies the grant against its own
    // key/identifiers using the SDK's pure claim-level check. Rule 1 (JWT
    // signature over the issuer's JWKS) is exercised implicitly in step 5,
    // where the gateway verifies the same token against its own StaticJwks.
    // ------------------------------------------------------------------
    let auth_claims_raw = decode_jwt_payload_unverified(&grant.auth_token);
    let auth_claims: trogon_identity_types::aauth::AuthClaims =
        serde_json::from_value(auth_claims_raw).expect("decode auth claims");
    let verified_response = verify_auth_claims(
        &auth_claims,
        ps_iss,       // resource_token_aud: the PS's own iss, matching the challenge's aud
        resource_iss, // intended_resource: the resource (gateway) the agent means to access
        &agent_jwk,
        agent_id.as_str(),
        None,
    )
    .expect("auth token response verification (rules 2-6) succeeds");
    assert!(
        verified_response.upstream_agent.is_none(),
        "direct grant carries no delegation chain"
    );
    assert_eq!(auth_claims.scope, GRANTED_SCOPE);

    // ------------------------------------------------------------------
    // Step 5 -- Authorized presentation ("Scopes"): sign a fresh NATS
    // request with the auth token attached, for a method the granted scope
    // covers. The gateway must resolve it, asserting the PS-asserted
    // access.
    // ------------------------------------------------------------------
    let authorized_signer = AgentSigner::new(agent_signing.clone(), agent_jwt.clone())
        .expect("agent signer")
        .with_auth_token(grant.auth_token.clone());
    let authorized_headers = authorized_signer
        .sign_nats_request_now(subject, Some(reply), &payload)
        .into_pairs();

    let resolution = ingress
        .resolve_nats(
            subject,
            Some(reply),
            &payload,
            &authorized_headers,
            Some(&grant.auth_token),
            COVERED_METHOD,
        )
        .await
        .expect("covered method resolves under the granted scope");

    assert_eq!(resolution.agent_id.as_deref(), Some(agent_id.as_str()));
    assert_eq!(resolution.agent_jkt.as_deref(), Some(agent_jkt_expected.as_str()));
    assert_eq!(resolution.scope.as_deref(), Some(GRANTED_SCOPE));
    assert!(
        resolution.auth_jti.is_some(),
        "resolution must carry the auth token's jti"
    );
    // NOTE on `resolution.principal`: the draft's three-party mode lets a PS
    // assert a directed `principal` on the auth token (`AuthClaims::principal`,
    // `AuthTokenInputs::principal` in trogon-aauth-person/src/mint.rs), and
    // `AAuthResolution::attach_auth` does thread it through. But
    // `PersonServer::apply_decision`'s `PolicyDecision::Grant` arm
    // (trogon-aauth-person/src/server.rs) hard-codes `principal: None` when
    // minting, and `PolicyDecision::Grant { scope }` has no field for a
    // policy engine to supply one. There is no public seam in
    // trogon-aauth-person to produce a PS grant with a non-None principal --
    // see this test's final report for the composability note. The most we
    // can assert here is that today's PS always mints `principal: None`.
    assert_eq!(
        resolution.principal, None,
        "trogon-aauth-person's PolicyDecision::Grant has no principal field; PersonServer always mints principal: None (see mismatch note above)"
    );

    // ------------------------------------------------------------------
    // Step 6 -- Negative coda ("Scopes"): the same auth token, presented
    // against a method the granted scope does NOT cover, must deny with
    // ScopeNotCovered rather than a fresh PoP challenge -- the agent already
    // has a valid session, it just needs a broader grant.
    // ------------------------------------------------------------------
    let uncovered_subject = "a2a.gateway.bot.tasks.get";
    let uncovered_headers = authorized_signer
        .sign_nats_request_now(uncovered_subject, Some(reply), &payload)
        .into_pairs();
    let scope_deny = ingress
        .resolve_nats(
            uncovered_subject,
            Some(reply),
            &payload,
            &uncovered_headers,
            Some(&grant.auth_token),
            UNCOVERED_METHOD,
        )
        .await
        .expect_err("uncovered method must deny even with a valid auth token");
    assert_eq!(scope_deny.code, AAUTH_REQUIRED_CODE);
    assert!(
        matches!(scope_deny.reason, AAuthDenyReason::ScopeNotCovered { .. }),
        "expected ScopeNotCovered, got {:?}",
        scope_deny.reason
    );
}

/// Decodes a JWT's payload segment into raw JSON without verifying its
/// signature. Used only where this test wants to inspect claims minted by a
/// party whose key wasn't published into this specific decoder's JWKS view
/// (the challenge is gateway-self-signed with a key the test never
/// publishes; the PS response's rule-1 signature check happens for real in
/// step 5 via the gateway's own `resolve_nats`).
fn decode_jwt_payload_unverified(jwt: &str) -> serde_json::Value {
    let payload_b64 = jwt.split('.').nth(1).expect("jwt has a payload segment");
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .expect("base64url decode");
    serde_json::from_slice(&payload).expect("payload is valid json")
}

const _: () = {
    // Referenced only to document, at a glance, which replay-store type
    // backs the gateway ingress built in this test (InMemoryReplayStore via
    // `AAuthIngress::new_in_memory`), mirroring aauth_roundtrip.rs's
    // self-documentation idiom for the same import.
    fn _type_witness(_: InMemoryReplayStore) {}
};
