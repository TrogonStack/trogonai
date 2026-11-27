use std::time::Duration;

use oci_client::Reference;
use oci_client::manifest::OciDescriptor;
use oci_client::secrets::RegistryAuth;
use tokio::time::Instant;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{BoundedRegistry, authorization};
use crate::{OciError, RegistryHost};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn rejection(error: &OciError) -> Result<&str, Box<dyn std::error::Error>> {
    match error {
        OciError::RegistryRejected(message) => Ok(message),
        other => Err(format!("expected a registry rejection, got {other:?}").into()),
    }
}

fn registry(address: &str, deadline: Duration) -> Result<BoundedRegistry, Box<dyn std::error::Error>> {
    let reference: Reference = format!("{address}/acme/app:latest").parse()?;
    let insecure = [RegistryHost::new(address)?];
    let transport = BoundedRegistry::new(
        &reference,
        RegistryAuth::Anonymous,
        &insecure,
        Instant::now() + deadline,
    )?;
    Ok(transport)
}

fn descriptor(size: i64, urls: Option<Vec<String>>) -> OciDescriptor {
    OciDescriptor {
        media_type: "application/octet-stream".into(),
        digest: format!("sha256:{}", "0".repeat(64)),
        size,
        urls,
        annotations: None,
        artifact_type: None,
    }
}

#[test]
fn authorization_rejects_values_that_cannot_be_header_values() {
    assert!(authorization(&RegistryAuth::Bearer("token\n".into())).is_err());
}

#[tokio::test]
async fn blob_descriptors_with_foreign_urls_or_oversized_sizes_are_refused() -> TestResult {
    let mut transport = registry("127.0.0.1:1", Duration::from_secs(5))?;
    let mut sink = Vec::new();
    let foreign = descriptor(1, Some(vec!["https://fixture.invalid/blob".into()]));
    assert!(transport.blob(&foreign, 10, &mut sink).await.is_err());
    assert!(transport.blob(&descriptor(11, None), 10, &mut sink).await.is_err());
    Ok(())
}

#[tokio::test]
async fn slow_responses_hit_the_deadline() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let mut transport = registry(&server.address().to_string(), Duration::from_millis(150))?;
    let error = transport.manifest("latest").await.err().ok_or("must fail")?;
    assert!(rejection(&error)?.contains("deadline exceeded"), "{error}");
    Ok(())
}

#[tokio::test]
async fn unreachable_registries_fail_without_leaking_the_url() -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    drop(listener);
    let mut transport = registry(&address, Duration::from_secs(5))?;
    let error = transport.manifest("latest").await.err().ok_or("must fail")?;
    assert!(rejection(&error)?.contains("transport failed"), "{error}");
    assert!(!rejection(&error)?.contains(&address), "{error}");
    Ok(())
}
