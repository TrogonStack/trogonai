use super::*;

fn make_registry() -> ApiKeyRegistry {
    ApiKeyRegistry::new(b"test-hmac-secret".to_vec())
}

fn sample_entry() -> ApiKeyEntry {
    ApiKeyEntry {
        spicedb_principal: SpiceDbPrincipal::new("user/alice"),
        audience: AudienceAccount::new("nats-acct-1"),
        external_subject: ExternalSubject::new("alice@example.com").unwrap(),
    }
}

#[tokio::test]
async fn verify_unknown_key_returns_credential_error() {
    let registry = Arc::new(make_registry());
    #[allow(deprecated)]
    let verifier = HmacApiKeyVerifier::new(registry);
    let aud = AudienceAccount::new("nats-acct-1");
    #[allow(deprecated)]
    let err = verifier.verify("not-registered", &aud).await.unwrap_err();
    assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
}

#[tokio::test]
async fn verify_known_key_yields_expected_claims() {
    let mut registry = make_registry();
    let key = ApiKey::new("my-secret-key").unwrap();
    let entry = sample_entry();
    registry.register(&key, entry);
    let registry = Arc::new(registry);
    #[allow(deprecated)]
    let verifier = HmacApiKeyVerifier::new(registry);
    let aud = AudienceAccount::new("nats-acct-1");
    #[allow(deprecated)]
    let claims = verifier.verify("my-secret-key", &aud).await.unwrap();
    assert_eq!(claims.sub.as_str(), "alice@example.com");
    assert_eq!(claims.aud.as_str(), "nats-acct-1");
    assert_eq!(claims.data.0["spicedb_subject"], "user/alice");
}

#[tokio::test]
async fn verify_rejects_audience_mismatch() {
    let mut registry = make_registry();
    let key = ApiKey::new("my-secret-key").unwrap();
    registry.register(&key, sample_entry());
    let registry = Arc::new(registry);
    #[allow(deprecated)]
    let verifier = HmacApiKeyVerifier::new(registry);
    let other = AudienceAccount::new("nats-acct-evil");
    #[allow(deprecated)]
    let err = verifier.verify("my-secret-key", &other).await.unwrap_err();
    assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
    assert!(err.to_string().contains("audience mismatch"));
}

#[test]
fn apikey_rejects_empty() {
    let err = ApiKey::new("").unwrap_err();
    assert!(matches!(err, ApiKeyError::Empty));
    assert!(err.to_string().contains("empty"));
}

#[test]
fn registry_collides_on_double_register() {
    let mut registry = make_registry();
    let key = ApiKey::new("shared-key").unwrap();
    let first = ApiKeyEntry {
        spicedb_principal: SpiceDbPrincipal::new("user/first"),
        audience: AudienceAccount::new("acct-a"),
        external_subject: ExternalSubject::new("first@example.com").unwrap(),
    };
    let second = ApiKeyEntry {
        spicedb_principal: SpiceDbPrincipal::new("user/second"),
        audience: AudienceAccount::new("acct-b"),
        external_subject: ExternalSubject::new("second@example.com").unwrap(),
    };
    registry.register(&key, first);
    registry.register(&key, second);
    let found = registry.lookup(&key).unwrap();
    assert_eq!(found.external_subject.as_str(), "second@example.com");
}

#[test]
fn digest_is_deterministic() {
    let registry = make_registry();
    let key = ApiKey::new("stable-key").unwrap();
    let d1 = ApiKeyDigest::compute(&key, &registry.hmac_secret);
    let d2 = ApiKeyDigest::compute(&key, &registry.hmac_secret);
    assert_eq!(d1, d2);
}
