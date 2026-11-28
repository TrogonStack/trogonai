use trogon_sandbox_providers::VolumeCacheEnv;
use trogon_std::env::InMemoryEnv;

#[test]
fn no_bucket_means_no_cache() {
    assert!(matches!(VolumeCacheEnv::from_env(&InMemoryEnv::new()), Ok(None)));
}

#[test]
fn the_region_defaults_and_the_rest_is_optional() {
    let env = InMemoryEnv::new();
    env.set("SANDBOX_VOLUME_CACHE_BUCKET", "cache");
    assert_eq!(
        VolumeCacheEnv::from_env(&env).ok().flatten(),
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
        VolumeCacheEnv::from_env(&env).ok().flatten(),
        Some(VolumeCacheEnv {
            bucket: "cache".to_string(),
            region: "eu-west-1".to_string(),
            endpoint: Some("http://127.0.0.1:9000".to_string()),
            kms_key_id: Some("key-1".to_string()),
        })
    );
}

#[cfg(unix)]
#[test]
fn a_setting_that_is_not_unicode_is_refused() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    use trogon_sandbox_providers::VolumeCacheError;
    use trogon_std::env::ReadEnv;

    struct RawEnv(&'static str);

    impl ReadEnv for RawEnv {
        fn var(&self, key: &str) -> Result<String, std::env::VarError> {
            if key == self.0 {
                Err(std::env::VarError::NotUnicode(OsString::from_vec(vec![0xff, 0xfe])))
            } else if key == "SANDBOX_VOLUME_CACHE_BUCKET" {
                Ok("cache".to_string())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        }
    }

    for variable in [
        "SANDBOX_VOLUME_CACHE_BUCKET",
        "SANDBOX_VOLUME_CACHE_REGION",
        "SANDBOX_VOLUME_CACHE_ENDPOINT",
        "SANDBOX_VOLUME_CACHE_KMS_KEY_ID",
    ] {
        let result = VolumeCacheEnv::from_env(&RawEnv(variable));
        assert!(
            matches!(result, Err(VolumeCacheError::InvalidSetting { variable: v, .. }) if v == variable),
            "{variable}"
        );
    }
}
