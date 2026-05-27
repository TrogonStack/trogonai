mod support;

use std::sync::Arc;

use support::*;
use trogon_sts::DEFAULT_MESH_ISSUER;
use trogon_sts::attestor::{AttestationPolicy, build_attestor};
use trogon_sts::audit::RecordingAuditPublisher;
use trogon_sts::cache::{JwksCache, RegistryCache, TrustBundleCache};
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::exchange::ExchangeService;
use trogon_sts::registry::InMemoryRegistry;
use trogon_sts::spicedb::NoOpSpiceDb;

#[tokio::test]
async fn empty_request_purpose_rejected_when_required() {
    let keys = shared_test_keys();
    let service = build_service_with_mode_and_purpose(
        keys,
        sample_registry_record(),
        ChainResolutionMode::Off,
        true,
    );
    let subject = mint_bootstrap_token(keys, bootstrap_claims(keys));
    let mut request = sample_exchange_request(&subject);
    request.purpose = "   ".into();
    let err = service.handle(request, None).await.expect_err("deny");
    assert_eq!(err.error, "access_denied");
    assert!(err.error_description.contains("purpose_missing"));
}

#[tokio::test]
async fn empty_bootstrap_purpose_claim_rejected_when_required() {
    let keys = shared_test_keys();
    let service = build_service_with_mode_and_purpose(
        keys,
        sample_registry_record(),
        ChainResolutionMode::Off,
        true,
    );
    let mut claims = bootstrap_claims(keys);
    claims.as_object_mut().expect("object").remove("purpose");
    let subject = mint_bootstrap_token(keys, claims);
    let request = sample_exchange_request(&subject);
    let err = service.handle(request, None).await.expect_err("deny");
    assert!(err.error_description.contains("purpose_missing"));
}

#[tokio::test]
async fn empty_bootstrap_claim_allowed_when_require_disabled() {
    let keys = shared_test_keys();
    let service = build_service_with_mode_and_purpose(
        keys,
        sample_registry_record(),
        ChainResolutionMode::Off,
        false,
    );
    let mut claims = bootstrap_claims(keys);
    claims.as_object_mut().expect("object").remove("purpose");
    let subject = mint_bootstrap_token(keys, claims);
    let request = sample_exchange_request(&subject);
    service.handle(request, None).await.expect("exchange ok");
}

#[tokio::test]
async fn purpose_missing_emits_deny_audit_reason() {
    let keys = shared_test_keys();
    let audit = Arc::new(RecordingAuditPublisher::new());
    let jwks = JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone());
    let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
    let attestor = build_attestor(trust.clone(), AttestationPolicy::Shadow);
    let registry = RegistryCache::new(InMemoryRegistry::new([sample_registry_record()]));
    let service = ExchangeService::new(
        DEFAULT_MESH_ISSUER.to_string(),
        keys.bootstrap_iss.clone(),
        jwks,
        trust,
        attestor,
        AttestationPolicy::Shadow,
        registry,
        keys.mesh_signer.clone(),
        Arc::clone(&audit),
        NoOpSpiceDb,
        ChainResolutionMode::Off,
        true,
    );
    let subject = mint_bootstrap_token(keys, bootstrap_claims(keys));
    let mut request = sample_exchange_request(&subject);
    request.purpose.clear();
    let _ = service.handle(request, None).await;
    let deny = audit
        .take_events()
        .into_iter()
        .find(|e| e.outcome == "deny")
        .expect("deny audit");
    assert_eq!(deny.decision_reason, "purpose_missing");
}

#[test]
fn require_purpose_flag_defaults_true() {
    assert!(trogon_sts::exchange::parse_require_purpose_flag("true"));
    assert!(trogon_sts::exchange::parse_require_purpose_flag(""));
}

#[test]
fn require_purpose_flag_false_when_disabled() {
    assert!(!trogon_sts::exchange::parse_require_purpose_flag("false"));
    assert!(!trogon_sts::exchange::parse_require_purpose_flag("off"));
    assert!(!trogon_sts::exchange::parse_require_purpose_flag("0"));
}
