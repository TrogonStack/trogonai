use std::error::Error;

use super::{config, key};
use crate::config::{SANDBOX_CLASS_LABEL, SIZE_CLASS_LABEL};
use crate::template::build_template::build_template;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn the_template_runs_the_agent_with_a_wakeup_probe_on_the_ingress_port() -> TestResult {
    let key = key()?;
    let template = build_template(&config()?, "tenant-a", &key);
    let container = template.containers.first().ok_or("container")?;
    assert_eq!(container.image, key.image_reference());
    assert_eq!(container.command, ["/usr/local/bin/sandbox-actor-agent"]);
    assert_eq!(container.args, ["--port=8110"]);
    assert!(container.env.is_empty(), "per-lease env must not be frozen in");
    let http_get = container
        .wakeup_probe
        .as_ref()
        .and_then(|probe| probe.http_get.as_ref())
        .ok_or("wakeup probe")?;
    assert_eq!(http_get.port, 8110);
    assert_eq!(http_get.path, trogon_sandbox_actor_agent::READYZ_PATH);
    assert_eq!(template.volumes.len(), 1);
    assert_eq!(
        template.metadata.ok_or("metadata")?.name,
        key.template_name(),
        "the name is the content address"
    );
    Ok(())
}

#[test]
fn the_template_carries_a_worker_selector_so_the_golden_build_lands_on_the_right_pool() -> TestResult {
    // Without this, the golden build is unconstrained and can land on any pool in the
    // cluster, including one that has nothing to do with this tenant's leases.
    let template = build_template(&config()?, "tenant-a", &key()?);
    let selector = template.worker_selector.ok_or("worker selector")?;
    assert_eq!(selector.match_labels.get(SIZE_CLASS_LABEL), Some(&"small".to_string()));
    assert_eq!(
        selector.match_labels.get(SANDBOX_CLASS_LABEL),
        Some(&"gvisor".to_string())
    );
    Ok(())
}

#[test]
fn the_template_carries_the_resource_limits_the_key_was_built_from() -> TestResult {
    let template = build_template(&config()?, "tenant-a", &key()?);
    let limits = template
        .resources
        .and_then(|resources| resources.limits)
        .ok_or("limits")?;
    assert_eq!(limits.cpu_millicores, 1000);
    Ok(())
}
