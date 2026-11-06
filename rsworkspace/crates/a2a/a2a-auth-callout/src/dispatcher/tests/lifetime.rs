use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;

use super::*;
use crate::credentials::mtls::{TrustAnchorPem, X509MtlsVerifier};
use crate::credentials::oidc::{JwksOidcVerifier, OidcIssuerUrl};

fn oidc(expiry_offset: i64) -> (Arc<dyn OidcVerifier>, String, i64) {
    oidc_with_not_before(expiry_offset, None)
}

fn oidc_with_not_before(expiry_offset: i64, not_before_offset: Option<i64>) -> (Arc<dyn OidcVerifier>, String, i64) {
    oidc_with_not_before_claim(expiry_offset, |now| not_before_offset.map(|offset| json!(now + offset)))
}

fn oidc_with_not_before_claim(
    expiry_offset: i64,
    not_before: impl FnOnce(i64) -> Option<serde_json::Value>,
) -> (Arc<dyn OidcVerifier>, String, i64) {
    oidc_with_header(expiry_offset, not_before, |_| {})
}

fn oidc_with_header(
    expiry_offset: i64,
    not_before: impl FnOnce(i64) -> Option<serde_json::Value>,
    configure_header: impl FnOnce(&mut serde_json::Value),
) -> (Arc<dyn OidcVerifier>, String, i64) {
    let key = RsaPrivateKey::new(&mut rand_core::OsRng, 2048).unwrap();
    let public = key.to_public_key();
    let jwks = serde_json::from_value(json!({"keys": [{
        "kty": "RSA", "kid": "lifetime-oidc", "use": "sig", "alg": "RS256",
        "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
        "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
    }]}))
    .unwrap();
    let encoding =
        jsonwebtoken::EncodingKey::from_rsa_pem(key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap().as_bytes())
            .unwrap();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let expires_at = now + expiry_offset;
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("lifetime-oidc".to_string());
    let mut claims =
        json!({"iss": "https://issuer.example", "aud": "callout", "sub": "person:alice", "exp": expires_at});
    if let Some(value) = not_before(now) {
        claims["nbf"] = value;
    }
    let mut header = serde_json::to_value(header).unwrap();
    configure_header(&mut header);
    let message = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap()),
    );
    let signature = jsonwebtoken::crypto::sign(message.as_bytes(), &encoding, jsonwebtoken::Algorithm::RS256).unwrap();
    let token = format!("{message}.{signature}");
    let verifier = JwksOidcVerifier::with_static_jwks(
        OidcIssuerUrl::parse("https://issuer.example").unwrap(),
        vec!["callout".to_string()],
        jwks,
    );
    (Arc::new(verifier), token, expires_at)
}

struct CertificateLifetimes {
    leaf: i64,
    intermediate: Option<i64>,
    anchor: i64,
}

fn certificate(lifetimes: CertificateLifetimes) -> (Arc<dyn MTlsVerifier>, Vec<String>, i64) {
    let now = OffsetDateTime::now_utc();
    let mut root_params = CertificateParams::default();
    root_params.distinguished_name = DistinguishedName::new();
    root_params.distinguished_name.push(DnType::CommonName, "lifetime-root");
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    root_params.not_before = now - time::Duration::seconds(10);
    root_params.not_after = now + time::Duration::seconds(lifetimes.anchor);
    let root_key = KeyPair::generate().unwrap();
    let root = root_params.self_signed(&root_key).unwrap();
    let root_issuer = Issuer::from_params(&root_params, &root_key);
    let mut leaf_params = CertificateParams::default();
    leaf_params.distinguished_name = DistinguishedName::new();
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, "lifetime-service");
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    leaf_params.not_before = now - time::Duration::seconds(10);
    leaf_params.not_after = now + time::Duration::seconds(lifetimes.leaf);
    let leaf_key = KeyPair::generate().unwrap();
    let chain = match lifetimes.intermediate {
        Some(ttl) => {
            let mut intermediate_params = root_params.clone();
            intermediate_params.distinguished_name = DistinguishedName::new();
            intermediate_params
                .distinguished_name
                .push(DnType::CommonName, "lifetime-intermediate");
            intermediate_params.not_after = now + time::Duration::seconds(ttl);
            let intermediate_key = KeyPair::generate().unwrap();
            let intermediate = intermediate_params.signed_by(&intermediate_key, &root_issuer).unwrap();
            let issuer = Issuer::from_params(&intermediate_params, &intermediate_key);
            let leaf = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
            vec![leaf.pem(), intermediate.pem()]
        }
        None => vec![leaf_params.signed_by(&leaf_key, &root_issuer).unwrap().pem()],
    };
    let expiry_offset = lifetimes
        .leaf
        .min(lifetimes.anchor)
        .min(lifetimes.intermediate.unwrap_or(i64::MAX));
    let verifier = X509MtlsVerifier::new(TrustAnchorPem::new(root.pem()));
    (Arc::new(verifier), chain, now.unix_timestamp() + expiry_offset)
}

#[tokio::test]
async fn dispatch_does_not_outlive_oidc_source_expiry() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, token, expires_at) = oidc(90);
    let mut config = config_with(Some(verifier), None, None, &[&tenant]);
    config.user_jwt_ttl = Duration::from_secs(3600);
    let dispatcher = CalloutDispatcher::new(config);
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some(token.clone());
    });
    let issued = dispatcher.dispatch(request).await.unwrap();
    let payload = decode_nats_user_payload(issued.as_str()).unwrap();
    assert!(
        payload["exp"].as_i64().unwrap() <= expires_at,
        "broker admission outlived its OIDC credential"
    );
}

#[tokio::test]
async fn dispatch_refuses_oidc_caller_without_a_tenant_membership_binding() {
    let tenant = Tenant::generate();
    let other = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, token, _) = oidc(7200);
    let mut config = config_with(Some(verifier), None, None, &[&tenant, &other]);
    config.tenant_admission = Arc::new(fixture_memberships(&[&tenant]));
    let dispatcher = CalloutDispatcher::new(config);
    assert!(
        dispatcher
            .dispatch(tenant.request(&server, |claims| {
                claims.nats.connect_opts.auth_token = Some(token.clone());
            }))
            .await
            .is_ok()
    );
    let request = other.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some(token.clone());
    });
    assert!(
        dispatcher.dispatch(request).await.is_err(),
        "a verified OIDC identity and a configured tenant signing key do not prove membership"
    );
}

#[tokio::test]
async fn dispatch_refuses_mtls_caller_without_a_tenant_membership_binding() {
    let tenant = Tenant::generate();
    let other = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, chain, _) = certificate(CertificateLifetimes {
        leaf: 1800,
        intermediate: None,
        anchor: 3600,
    });
    let chain: Vec<_> = chain.iter().map(String::as_str).collect();
    let mut config = config_with(None, Some(verifier), None, &[&tenant, &other]);
    config.tenant_admission = Arc::new(fixture_memberships(&[&tenant]));
    let dispatcher = CalloutDispatcher::new(config);
    let request = |tenant: &Tenant| {
        tenant.request(&server, |claims| {
            claims.nats.connect_opts.auth_token = None;
            claims.nats.client_tls = Some(verified_client_tls(&chain));
        })
    };
    assert!(dispatcher.dispatch(request(&tenant)).await.is_ok());
    assert!(matches!(
        dispatcher.dispatch(request(&other)).await,
        Err(AuthCalloutError::TenantAdmission(
            crate::tenant_admission::TenantAdmissionError::NotMember
        ))
    ));
}

#[tokio::test]
async fn dispatch_refuses_expired_oidc_source() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, token, _) = oidc(-1);
    let dispatcher = CalloutDispatcher::new(config_with(Some(verifier), None, None, &[&tenant]));
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some(token.clone());
    });
    assert!(
        dispatcher.dispatch(request).await.is_err(),
        "expired source must not authorize new broker admission"
    );
}

async fn assert_certificate_bound(lifetimes: CertificateLifetimes, configured_ttl: Duration) {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, chain, expires_at) = certificate(lifetimes);
    let chain: Vec<_> = chain.iter().map(String::as_str).collect();
    let mut config = config_with(None, Some(verifier), None, &[&tenant]);
    config.user_jwt_ttl = configured_ttl;
    let dispatcher = CalloutDispatcher::new(config);
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = None;
        claims.nats.connect_opts.pass = None;
        claims.nats.client_tls = Some(verified_client_tls(&chain));
    });
    let issued = dispatcher.dispatch(request).await.unwrap();
    let payload = decode_nats_user_payload(issued.as_str()).unwrap();
    let expiry = payload["exp"].as_i64().unwrap();
    assert!(
        expiry <= expires_at,
        "broker admission outlived its verified certificate path"
    );
    assert!(expiry - payload["iat"].as_i64().unwrap() <= i64::try_from(configured_ttl.as_secs()).unwrap());
}

#[tokio::test]
async fn dispatch_does_not_outlive_client_certificate() {
    assert_certificate_bound(
        CertificateLifetimes {
            leaf: 90,
            intermediate: None,
            anchor: 3600,
        },
        Duration::from_secs(3600),
    )
    .await;
}

#[tokio::test]
async fn dispatch_does_not_outlive_validated_intermediate() {
    assert_certificate_bound(
        CertificateLifetimes {
            leaf: 1800,
            intermediate: Some(90),
            anchor: 3600,
        },
        Duration::from_secs(3600),
    )
    .await;
}

#[tokio::test]
async fn dispatch_does_not_outlive_selected_trust_anchor() {
    assert_certificate_bound(
        CertificateLifetimes {
            leaf: 1800,
            intermediate: None,
            anchor: 90,
        },
        Duration::from_secs(3600),
    )
    .await;
}

#[tokio::test]
async fn dispatch_obeys_shorter_configured_lifetime() {
    assert_certificate_bound(
        CertificateLifetimes {
            leaf: 1800,
            intermediate: None,
            anchor: 3600,
        },
        Duration::from_secs(60),
    )
    .await;
}

#[derive(Clone)]
struct Clock(Arc<Mutex<SystemTime>>);

impl Clock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(SystemTime::now())))
    }

    fn set(&self, time: SystemTime) {
        *self.0.lock().unwrap() = time;
    }
}

impl EpochClock for Clock {
    fn system_time(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

struct ExpiringOidcVerifier {
    inner: Arc<dyn OidcVerifier>,
    clock: Clock,
    expires_at: i64,
}

#[async_trait::async_trait]
impl OidcVerifier for ExpiringOidcVerifier {
    async fn verify(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError> {
        let verified = self.inner.verify(token, account).await?;
        self.clock
            .set(UNIX_EPOCH + Duration::from_secs(u64::try_from(self.expires_at).unwrap()));
        Ok(verified)
    }
}

#[tokio::test]
async fn dispatch_rechecks_source_expiry_immediately_before_mint() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, token, expires_at) = oidc(90);
    let clock = Clock::new();
    let verifier = Arc::new(ExpiringOidcVerifier {
        inner: verifier,
        clock: clock.clone(),
        expires_at,
    });
    let dispatcher = CalloutDispatcher::new(config_with(Some(verifier), None, None, &[&tenant])).with_clock(clock);
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some(token.clone());
    });
    assert!(
        dispatcher.dispatch(request).await.is_err(),
        "verification success must not outlive its source credential"
    );
}

async fn assert_not_before_refused(offset: i64) {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, token, _) = oidc_with_not_before(7200, Some(offset));
    let dispatcher = CalloutDispatcher::new(config_with(Some(verifier), None, None, &[&tenant]));
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some(token.clone());
    });
    assert!(
        dispatcher.dispatch(request).await.is_err(),
        "future source validity must not authorize broker admission"
    );
}

#[tokio::test]
async fn dispatch_refuses_oidc_before_its_not_before() {
    assert_not_before_refused(3600).await;
}

#[tokio::test]
async fn dispatch_refuses_oidc_before_not_before_within_legacy_clock_tolerance() {
    assert_not_before_refused(30).await;
}

async fn assert_malformed_not_before_refused(value: serde_json::Value) {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, token, _) = oidc_with_not_before_claim(7200, |_| Some(value));
    let dispatcher = CalloutDispatcher::new(config_with(Some(verifier), None, None, &[&tenant]));
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some(token.clone());
    });
    assert!(
        dispatcher.dispatch(request).await.is_err(),
        "present malformed validity must not become an absent claim"
    );
}

#[tokio::test]
async fn dispatch_refuses_null_not_before() {
    assert_malformed_not_before_refused(serde_json::Value::Null).await;
}

#[tokio::test]
async fn dispatch_refuses_string_not_before() {
    assert_malformed_not_before_refused(json!("tomorrow")).await;
}

#[tokio::test]
async fn dispatch_refuses_fractional_not_before() {
    assert_malformed_not_before_refused(json!(1.5)).await;
}

#[tokio::test]
async fn dispatch_refuses_overflowing_not_before() {
    assert_malformed_not_before_refused(json!(u64::MAX)).await;
}

async fn assert_critical_header_refused(value: serde_json::Value) {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let (verifier, token, _) = oidc_with_header(
        7200,
        |_| None,
        |header| {
            header["crit"] = value;
            header["enterprise-policy"] = json!(true);
        },
    );
    let dispatcher = CalloutDispatcher::new(config_with(Some(verifier), None, None, &[&tenant]));
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some(token.clone());
    });
    assert!(
        dispatcher.dispatch(request).await.is_err(),
        "unsupported critical protected headers must not authorize broker admission"
    );
}

#[tokio::test]
async fn dispatch_refuses_unsupported_critical_header() {
    assert_critical_header_refused(json!(["enterprise-policy"])).await;
}

#[tokio::test]
async fn dispatch_refuses_empty_critical_header() {
    assert_critical_header_refused(json!([])).await;
}

#[tokio::test]
async fn dispatch_refuses_null_critical_header() {
    assert_critical_header_refused(serde_json::Value::Null).await;
}

#[tokio::test]
async fn dispatch_refuses_malformed_critical_header() {
    assert_critical_header_refused(json!("enterprise-policy")).await;
}
