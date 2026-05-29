//! Integration: publish trust bundle to KV, exchange with X.509 SVID actor_token.

mod support;

use std::sync::Arc;

use support::*;
use trogon_sts::attestor::{AttestationPolicy, X509SvidAttestor, build_attestor};
use trogon_sts::audit::RecordingAuditPublisher;
use trogon_sts::cache::{JwksCache, RegistryCache, TrustBundleCache, load_trust_bundle_from_kv};
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::exchange::ExchangeService;
use trogon_sts::registry::InMemoryRegistry;
use trogon_sts::spicedb::NoOpSpiceDb;
use trogon_sts::{DEFAULT_MESH_ISSUER, TRUST_BUNDLES_KV_BUCKET};

fn nats_server_available() -> bool {
    std::process::Command::new("nats-server")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn x509_svid_actor_token_mints_attested_wkl() {
    let (leaf_pem, anchor_pem) =
        test_spiffe_svid_pem("acme.local", "ns/prod/sa/oncall-agent");
    let keys = shared_test_keys();
    let trust = TrustBundleCache::from_pem_for_domain("acme.local", anchor_pem);
    let attestor = Arc::new(X509SvidAttestor::new(trust.clone()));
    let registry = RegistryCache::new(InMemoryRegistry::new([sample_registry_record()]));
    let audit = Arc::new(RecordingAuditPublisher::new());
    let service = ExchangeService::new(
        DEFAULT_MESH_ISSUER.to_string(),
        keys.bootstrap_iss.clone(),
        JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone()),
        trust,
        attestor,
        AttestationPolicy::Require,
        registry,
        keys.mesh_signer.clone(),
        audit.clone(),
        NoOpSpiceDb,
        ChainResolutionMode::Off,
        true,
    );

    let subject = mint_bootstrap_token(keys, bootstrap_claims(keys));
    let mut request = sample_exchange_request(&subject);
    request.actor_token = leaf_pem;

    let response = service.handle(request, None).await.expect("exchange");
    let payload = decode_payload(&response.access_token);
    assert_eq!(
        payload.get("wkl").and_then(|v| v.as_str()),
        Some("spiffe://acme.local/ns/prod/sa/oncall-agent")
    );
    let attested_at = payload
        .get("wkl_attested_at")
        .and_then(|v| v.as_i64())
        .expect("wkl_attested_at claim is minted alongside wkl");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    assert!(
        attested_at > 0 && (now - attested_at).abs() < 30,
        "wkl_attested_at should reflect a recent unix second; got {attested_at}, now {now}"
    );

    let events = audit.take_events();
    assert!(events.iter().any(|e| e.outcome == "success"));
    assert!(!events.iter().any(|e| e.decision_reason == "wkl_unattested"));
}

#[tokio::test]
async fn kv_trust_bundle_load_enables_exchange() {
    if !nats_server_available() {
        eprintln!("skip: nats-server not on PATH");
        return;
    }
    let Some((_child, nats)) = spawn_nats_js().await else {
        eprintln!("skip: could not start nats-server");
        return;
    };

    let (leaf_pem, anchor_pem) =
        test_spiffe_svid_pem("acme.local", "ns/prod/sa/oncall-agent");

    let js = async_nats::jetstream::new(nats.clone());
    let store = js
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: TRUST_BUNDLES_KV_BUCKET.to_string(),
            ..Default::default()
        })
        .await
        .expect("kv");
    store
        .put("acme.local", anchor_pem.into())
        .await
        .expect("put bundle");

    let (domain, pem) = load_trust_bundle_from_kv(&nats, TRUST_BUNDLES_KV_BUCKET, "acme.local")
        .await
        .expect("load");
    assert_eq!(domain, "acme.local");

    let keys = shared_test_keys();
    let trust = TrustBundleCache::from_pem_for_domain("local", String::new());
    trust.upsert_domain(domain, pem).await;
    let attestor = build_attestor(trust.clone(), AttestationPolicy::Require);
    let registry = RegistryCache::new(InMemoryRegistry::new([sample_registry_record()]));
    let service = ExchangeService::new(
        DEFAULT_MESH_ISSUER.to_string(),
        keys.bootstrap_iss.clone(),
        JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone()),
        trust,
        attestor,
        AttestationPolicy::Require,
        registry,
        keys.mesh_signer.clone(),
        Arc::new(RecordingAuditPublisher::new()),
        NoOpSpiceDb,
        ChainResolutionMode::Off,
        true,
    );

    let subject = mint_bootstrap_token(keys, bootstrap_claims(keys));
    let mut request = sample_exchange_request(&subject);
    request.actor_token = leaf_pem;
    service.handle(request, None).await.expect("exchange");
}
