use std::error::Error;
use std::time::{Duration, UNIX_EPOCH};

use trogon_sandbox_auth::{
    ApiKeyBucket, ApiKeyBucketError, ApiKeyHash, ApiKeyHashError, ApiKeyRecord, ApiKeyRecordError, ApiKeyStore,
    ApiKeyStoreError, BearerToken, BearerTokenError, DEFAULT_BUCKET, InMemoryApiKeyStore,
};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

type TestResult = Result<(), Box<dyn Error>>;

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn record() -> Result<ApiKeyRecord, Box<dyn Error>> {
    Ok(ApiKeyRecord::new(
        TenantId::new("tenant-a")?,
        ServiceAccountId::new("service-a"),
        "ci",
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    ))
}

#[test]
fn default_bucket_uses_the_sandbox_namespace() -> TestResult {
    assert_eq!(DEFAULT_BUCKET, "sandbox-api-keys");
    assert_eq!(ApiKeyBucket::default().as_str(), DEFAULT_BUCKET);
    assert_eq!(ApiKeyBucket::default().to_string(), DEFAULT_BUCKET);
    Ok(())
}

#[test]
fn bucket_accepts_letters_digits_hyphens_and_underscores() -> TestResult {
    let bucket: ApiKeyBucket = "tenant-key_store9".parse()?;
    assert_eq!(bucket.as_str(), "tenant-key_store9");
    assert_eq!(ApiKeyBucket::new("keys")?, "keys".parse()?);
    Ok(())
}

#[test]
fn bucket_rejects_names_the_key_value_store_cannot_hold() {
    assert_eq!(ApiKeyBucket::new(""), Err(ApiKeyBucketError::Empty));
    for value in ["keys.tenant", "keys/tenant", "keys*", " keys", "kéys"] {
        assert_eq!(
            ApiKeyBucket::new(value),
            Err(ApiKeyBucketError::InvalidCharacter),
            "{value:?}"
        );
    }
    assert!(ApiKeyBucketError::Empty.to_string().contains("empty"));
    assert!(ApiKeyBucketError::InvalidCharacter.to_string().contains("hyphens"));
}

#[test]
fn bearer_token_hash_is_lowercase_hex_sha256() -> TestResult {
    let token = BearerToken::new("abc")?;
    assert_eq!(token.expose_secret(), "abc");
    assert_eq!(token.hash().as_str(), ABC_SHA256);
    assert_eq!(token.hash(), ApiKeyHash::of(&token));
    assert_eq!(token.hash().to_string(), ABC_SHA256);
    Ok(())
}

#[test]
fn bearer_token_never_prints_its_value() -> TestResult {
    let token = BearerToken::new("super-secret-value")?;
    let debug = format!("{token:?}");
    assert_eq!(debug, "BearerToken(***)");
    assert!(!debug.contains("super-secret-value"));
    Ok(())
}

#[test]
fn bearer_token_rejects_empty_and_non_visible_characters() {
    assert!(matches!(BearerToken::new(""), Err(BearerTokenError::Empty)));
    for value in ["a b", "a\n", "tökén", "a\tb"] {
        assert!(
            matches!(BearerToken::new(value), Err(BearerTokenError::InvalidCharacter)),
            "{value:?}"
        );
    }
    assert!(BearerTokenError::Empty.to_string().contains("empty"));
    assert!(BearerTokenError::InvalidCharacter.to_string().contains("visible"));
}

#[test]
fn key_hash_accepts_only_64_lowercase_hex_characters() -> TestResult {
    assert_eq!(ApiKeyHash::new(ABC_SHA256)?.as_str(), ABC_SHA256);
    assert_eq!(ABC_SHA256.parse::<ApiKeyHash>()?.as_str(), ABC_SHA256);
    assert_eq!(ApiKeyHash::new("abc"), Err(ApiKeyHashError::WrongLength));
    assert_eq!(
        ApiKeyHash::new(ABC_SHA256.to_uppercase()),
        Err(ApiKeyHashError::InvalidCharacter)
    );
    assert_eq!(ApiKeyHash::new("g".repeat(64)), Err(ApiKeyHashError::InvalidCharacter));
    assert!(ApiKeyHashError::WrongLength.to_string().contains("64"));
    assert!(ApiKeyHashError::InvalidCharacter.to_string().contains("hexadecimal"));
    Ok(())
}

#[test]
fn record_json_shape_is_stable() -> TestResult {
    let json = r#"{"tenant":"tenant-a","service_account":"service-a","name":"ci","created_at":1700000000}"#;
    let parsed: ApiKeyRecord = serde_json::from_str(json)?;
    assert_eq!(parsed, record()?);
    assert_eq!(parsed.tenant().as_str(), "tenant-a");
    assert_eq!(parsed.service_account().as_str(), "service-a");
    assert_eq!(parsed.name(), "ci");
    assert_eq!(parsed.created_at(), UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    assert_eq!(serde_json::to_string(&parsed)?, json);
    Ok(())
}

#[test]
fn record_before_the_epoch_serializes_as_zero() -> TestResult {
    let record = ApiKeyRecord::new(
        TenantId::new("tenant-a")?,
        ServiceAccountId::new("service-a"),
        "ci",
        UNIX_EPOCH - Duration::from_secs(5),
    );
    let value: serde_json::Value = serde_json::to_value(&record)?;
    assert_eq!(value["created_at"], 0);
    Ok(())
}

#[test]
fn record_rejects_an_invalid_tenant() {
    let json = r#"{"tenant":"","service_account":"s","name":"n","created_at":1}"#;
    let error = serde_json::from_str::<ApiKeyRecord>(json).err().map(|e| e.to_string());
    assert!(error.is_some_and(|message| message.contains("tenant")));
    assert!(ApiKeyRecordError::CreatedAtOutOfRange.to_string().contains("range"));
}

#[test]
fn record_rejects_an_out_of_range_creation_time() {
    let json = format!(
        r#"{{"tenant":"t","service_account":"s","name":"n","created_at":{}}}"#,
        u64::MAX
    );
    let error = serde_json::from_str::<ApiKeyRecord>(&json).err().map(|e| e.to_string());
    assert!(error.is_some_and(|message| message.contains("out of range")));
}

#[tokio::test]
async fn in_memory_store_looks_up_by_hash() -> TestResult {
    let store = InMemoryApiKeyStore::new();
    let token = BearerToken::new("k")?;
    store.insert(token.hash(), record()?).await;
    assert_eq!(store.lookup(&token.hash()).await?, Some(record()?));
    assert_eq!(store.lookup(&BearerToken::new("other")?.hash()).await?, None);
    Ok(())
}

#[test]
fn store_errors_describe_the_failure_without_secrets() -> TestResult {
    let malformed = serde_json::from_str::<ApiKeyRecord>("{")
        .err()
        .ok_or("expected a parse error")?;
    let error = ApiKeyStoreError::from(malformed);
    assert!(error.to_string().starts_with("API key record is malformed"));
    let backend = ApiKeyStoreError::Backend("bucket unavailable".into());
    assert_eq!(backend.to_string(), "API key store backend error: bucket unavailable");
    Ok(())
}
