use std::collections::HashMap;
use std::env::VarError;
use std::error::Error;

use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_kubevirt_worker::{
    AWS_ACCESS_KEY_ID_ENV, AWS_SECRET_ACCESS_KEY_ENV, AWS_SESSION_TOKEN_ENV, RestoreStores, S3RestoreStores,
};
use trogon_sandbox_volumes::{VolumeEndpoint, VolumeRegion};
use trogon_std::env::ReadEnv;

type TestResult = Result<(), Box<dyn Error>>;

/// An environment that is thread safe, which the process-wide one the worker reads is.
#[derive(Default)]
struct MapEnv(HashMap<String, String>);

impl MapEnv {
    fn with(mut self, key: &str, value: String) -> Self {
        self.0.insert(key.to_owned(), value);
        self
    }
}

impl ReadEnv for MapEnv {
    fn var(&self, key: &str) -> Result<String, VarError> {
        self.0.get(key).cloned().ok_or(VarError::NotPresent)
    }
}

/// A credential value built at run time, so no secret-shaped literal lives in the source.
fn credential(part: &str) -> String {
    format!("{}-{}", part, "x".repeat(12))
}

fn open(env: MapEnv, endpoint: Option<&str>) -> TestResult {
    let stores = S3RestoreStores::new(env);
    let endpoint = endpoint.map(str::parse::<VolumeEndpoint>).transpose()?;
    let store = stores.open(
        &S3BucketName::new("restore-bucket")?,
        &VolumeRegion::new("eu-west-1")?,
        endpoint.as_ref(),
    )?;
    assert!(store.to_string().contains("restore-bucket"));
    Ok(())
}

#[test]
fn a_store_opens_without_any_credentials() -> TestResult {
    open(MapEnv::default(), None)
}

#[test]
fn a_store_opens_with_a_key_pair_and_a_session_token() -> TestResult {
    let env = MapEnv::default()
        .with(AWS_ACCESS_KEY_ID_ENV, credential("id"))
        .with(AWS_SECRET_ACCESS_KEY_ENV, credential("secret"))
        .with(AWS_SESSION_TOKEN_ENV, credential("token"));
    open(env, Some("https://s3.example.test"))
}

#[test]
fn half_a_key_pair_is_ignored_and_plain_http_endpoints_are_allowed() -> TestResult {
    let env = MapEnv::default().with(AWS_ACCESS_KEY_ID_ENV, credential("id"));
    open(env, Some("http://minio.local:9000"))
}
