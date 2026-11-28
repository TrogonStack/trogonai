use object_store::ClientConfigKey;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn region() -> Result<VolumeRegion, Box<dyn std::error::Error>> {
    Ok(VolumeRegion::new("us-east-1")?)
}

fn allow_http(builder: &AmazonS3Builder) -> Option<String> {
    builder.get_config_value(&AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp))
}

fn permissive_environment() -> AmazonS3Builder {
    AmazonS3Builder::new().with_config(AmazonS3ConfigKey::Client(ClientConfigKey::AllowHttp), "true")
}

#[test]
fn the_environment_cannot_allow_plain_http_without_an_http_endpoint() -> TestResult {
    let builder = s3_builder(permissive_environment(), "bucket", &region()?, None);
    assert_eq!(allow_http(&builder).as_deref(), Some("false"));

    let secure = VolumeEndpoint::new("https://s3.example")?;
    let builder = s3_builder(permissive_environment(), "bucket", &region()?, Some(&secure));
    assert_eq!(allow_http(&builder).as_deref(), Some("false"));
    Ok(())
}

#[test]
fn a_configured_http_endpoint_is_the_only_way_to_allow_plain_http() -> TestResult {
    let plain = VolumeEndpoint::new("http://127.0.0.1:9000")?;
    let builder = s3_builder(AmazonS3Builder::new(), "bucket", &region()?, Some(&plain));
    assert_eq!(allow_http(&builder).as_deref(), Some("true"));
    Ok(())
}
