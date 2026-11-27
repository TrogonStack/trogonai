//! Tests for the content-addressed template registry.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::SubstrateSizeClasses;

use super::{TemplateIndex, TemplateKey, TemplateRegistry};
use crate::config::{DEFAULT_ROUTER_ENDPOINT, SandboxClass, SubstrateConfig};
use crate::resources::ActorResources;

mod build_tests;
mod index_error_tests;
mod key_tests;
mod memory_index;
mod nats_index_tests;
mod record_tests;
mod registry_failure_tests;
mod registry_tests;
mod scripted_control_tests;

pub(super) const IMAGE: &str =
    "registry.example/img@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) const AGENT_IMAGE: &str =
    "registry.example/agent@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub(super) const DEADLINE: Duration = Duration::from_secs(5);

pub(super) fn config() -> Result<SubstrateConfig, Box<dyn Error>> {
    Ok(SubstrateConfig {
        ate_api_endpoint: "http://ate-api-server:8080".to_string(),
        router_endpoint: DEFAULT_ROUTER_ENDPOINT.parse()?,
        agent_image: AGENT_IMAGE.to_string(),
        agent_port: 8110,
        sandbox_class: SandboxClass::Gvisor,
        snapshots_location: "gs://bucket/snapshots".to_string(),
        size_classes: "small=2048,medium=8192,large=32768".parse::<SubstrateSizeClasses>()?,
        template_build_timeout: Duration::from_secs(600),
        resume_timeout: Duration::from_secs(60),
        max_concurrent_leases: 64,
        router_tls: None,
        actor_cpu_millicores: 1000,
    })
}

pub(super) fn key_with(
    image: &str,
    class: SandboxClass,
    agent: &str,
    port: u16,
    label: &str,
) -> Result<TemplateKey, Box<dyn Error>> {
    let config = SubstrateConfig {
        agent_image: agent.into(),
        agent_port: port,
        ..config()?
    };
    let memory = config
        .size_classes
        .as_slice()
        .iter()
        .find(|entry| entry.label() == label)
        .ok_or("unknown size class")?
        .memory_mib();
    Ok(TemplateKey::new(
        image,
        class,
        &config,
        label,
        ActorResources::new(1000, memory)?,
    ))
}

pub(super) fn key() -> Result<TemplateKey, Box<dyn Error>> {
    key_with(IMAGE, SandboxClass::Gvisor, AGENT_IMAGE, 8110, "small")
}

pub(super) fn registry(index: Arc<dyn TemplateIndex>) -> TemplateRegistry {
    TemplateRegistry::new(index).with_poll_interval(Duration::from_millis(1))
}
