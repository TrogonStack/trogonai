use super::*;
use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;

#[test]
fn init_provider_returns_valid_provider() {
    let resource = Resource::builder()
        .with_service_name("test-metric")
        .with_attributes(vec![KeyValue::new("test", "true")])
        .build();

    let provider = init_provider(&resource);
    assert!(provider.is_ok());
}

#[test]
fn shutdown_succeeds_when_provider_not_initialized() {
    assert!(shutdown().is_ok());
}
