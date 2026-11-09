use opentelemetry_sdk::Resource;
use trogon_std::env::ReadEnv;

use crate::{ResourceAttribute, ServiceName};

pub(crate) fn build<A>(service_name: ServiceName, resource_attributes: A, env: &impl ReadEnv) -> Resource
where
    A: IntoIterator<Item = ResourceAttribute>,
{
    let service_name = env
        .var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            env.var("OTEL_RESOURCE_ATTRIBUTES").ok().and_then(|attributes| {
                attributes.split_terminator(',').rev().find_map(|entry| {
                    let (key, value) = entry.split_once('=')?;
                    (key.trim() == "service.name").then(|| value.trim().to_owned())
                })
            })
        })
        .unwrap_or_else(|| service_name.as_str().to_owned());

    Resource::builder()
        .with_service_name(service_name)
        .with_attributes(resource_attributes.into_iter().map(opentelemetry::KeyValue::from))
        .build()
}

#[cfg(test)]
mod tests;
