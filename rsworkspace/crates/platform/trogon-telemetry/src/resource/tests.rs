use opentelemetry::{Key, Value};
use trogon_std::env::InMemoryEnv;

use super::*;

fn assert_service_name(env: &InMemoryEnv, expected: &str) {
    let resource = build(ServiceName::TrogonProviderGateway, [], env);
    assert_eq!(
        resource.get(&Key::new("service.name")),
        Some(Value::from(expected.to_owned()))
    );
}

#[test]
fn typed_service_name_is_the_fallback() {
    assert_service_name(&InMemoryEnv::new(), "trogon-provider-gateway");
}

#[test]
fn explicit_service_name_overrides_resource_attributes() {
    let env = InMemoryEnv::new();
    env.set("OTEL_SERVICE_NAME", "deployed-gateway");
    env.set("OTEL_RESOURCE_ATTRIBUTES", "service.name=attribute-gateway");

    assert_service_name(&env, "deployed-gateway");
}

#[test]
fn resource_attributes_supply_the_service_name() {
    let env = InMemoryEnv::new();
    env.set(
        "OTEL_RESOURCE_ATTRIBUTES",
        "deployment.environment.name=test, service.name = attribute-gateway ,service.namespace=provider",
    );

    assert_service_name(&env, "attribute-gateway");
}

#[test]
fn empty_service_name_variable_falls_through_to_resource_attributes() {
    let env = InMemoryEnv::new();
    env.set("OTEL_SERVICE_NAME", "");
    env.set("OTEL_RESOURCE_ATTRIBUTES", "service.name=attribute-gateway");

    assert_service_name(&env, "attribute-gateway");
}

#[test]
fn empty_service_name_variable_without_attribute_uses_typed_fallback() {
    let env = InMemoryEnv::new();
    env.set("OTEL_SERVICE_NAME", "");
    env.set("OTEL_RESOURCE_ATTRIBUTES", "service.namespace=provider,service.name");

    assert_service_name(&env, "trogon-provider-gateway");
}

#[test]
fn duplicate_resource_attributes_use_the_last_valid_assignment() {
    let env = InMemoryEnv::new();
    env.set(
        "OTEL_RESOURCE_ATTRIBUTES",
        "service.name=first,service.name=last=retained,service.name,",
    );

    assert_service_name(&env, "last=retained");
}

#[test]
fn an_explicitly_empty_service_name_attribute_is_preserved() {
    let env = InMemoryEnv::new();
    env.set("OTEL_RESOURCE_ATTRIBUTES", "service.name=first,service.name= ");

    assert_service_name(&env, "");
}

#[test]
fn explicit_service_name_value_is_not_trimmed() {
    let env = InMemoryEnv::new();
    env.set("OTEL_SERVICE_NAME", " deployed-gateway ");

    assert_service_name(&env, " deployed-gateway ");
}

#[test]
fn sdk_metadata_and_package_resource_attributes_remain_available() {
    let env = InMemoryEnv::new();
    env.set("OTEL_SERVICE_NAME", "deployed-gateway");
    let resource = build(
        ServiceName::TrogonProviderGateway,
        [ResourceAttribute::acp_prefix("test-prefix")],
        &env,
    );

    assert_eq!(
        resource.get(&Key::new("telemetry.sdk.language")),
        Some(Value::from("rust"))
    );
    assert_eq!(
        resource.get(&Key::new(crate::constants::RESOURCE_ATTRIBUTE_ACP_PREFIX)),
        Some(Value::from("test-prefix"))
    );
}
