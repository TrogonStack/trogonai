use trogon_sandbox_providers::VolumeCacheEnv;
use trogon_std::env::InMemoryEnv;

#[test]
fn no_bucket_means_no_cache() {
    assert_eq!(VolumeCacheEnv::from_env(&InMemoryEnv::new()), None);
}

#[test]
fn the_region_defaults_and_the_rest_is_optional() {
    let env = InMemoryEnv::new();
    env.set("SANDBOX_VOLUME_CACHE_BUCKET", "cache");
    assert_eq!(
        VolumeCacheEnv::from_env(&env),
        Some(VolumeCacheEnv {
            bucket: "cache".to_string(),
            region: "us-east-1".to_string(),
            endpoint: None,
            kms_key_id: None,
        })
    );
}

#[test]
fn every_setting_is_read() {
    let env = InMemoryEnv::new();
    env.set("SANDBOX_VOLUME_CACHE_BUCKET", "cache");
    env.set("SANDBOX_VOLUME_CACHE_REGION", "eu-west-1");
    env.set("SANDBOX_VOLUME_CACHE_ENDPOINT", "http://127.0.0.1:9000");
    env.set("SANDBOX_VOLUME_CACHE_KMS_KEY_ID", "key-1");
    assert_eq!(
        VolumeCacheEnv::from_env(&env),
        Some(VolumeCacheEnv {
            bucket: "cache".to_string(),
            region: "eu-west-1".to_string(),
            endpoint: Some("http://127.0.0.1:9000".to_string()),
            kms_key_id: Some("key-1".to_string()),
        })
    );
}
