use std::collections::{BTreeMap, HashMap};

use bollard::models::{PortBinding, PortMap};

use super::{BollardEngine, container_cpus, host_cpus, non_empty};
use crate::{ContainerHardening, ContainerRunSpec};

fn spec() -> ContainerRunSpec {
    ContainerRunSpec {
        name: "n".to_string(),
        image: "img:1".to_string(),
        memory_mib: 512,
        cpu: 2.0,
        disk_mib: None,
        env: vec![],
        binds: vec![],
        network_mode: "bridge".to_string(),
        entrypoint_override: None,
        cmd_override: None,
        working_dir: None,
        user: None,
        cap_add: vec![],
        cap_drop: vec![],
        exposed_ports: vec![],
        hardening: ContainerHardening::default(),
        labels: BTreeMap::new(),
    }
}

#[test]
fn cpus_are_clamped_to_the_host_and_a_floor() {
    assert_eq!(container_cpus(8.0, 4.0), 4.0);
    assert_eq!(container_cpus(0.0, 4.0), 0.1);
    assert_eq!(container_cpus(1.5, 4.0), 1.5);
    assert!(host_cpus() >= 1.0);
}

#[test]
fn non_empty_maps_empty_to_none() {
    assert_eq!(non_empty::<u8>(&[]), None);
    assert_eq!(non_empty(&[1u8, 2]), Some(vec![1, 2]));
}

#[test]
fn plain_spec_sets_memory_cpu_and_auto_remove() {
    let config = BollardEngine::container_config(&spec());
    let host = config.host_config.unwrap_or_default();
    assert_eq!(config.image.as_deref(), Some("img:1"));
    assert_eq!(host.memory, Some(512 * 1024 * 1024));
    assert_eq!(host.memory_swap, None);
    assert_eq!(host.network_mode.as_deref(), Some("bridge"));
    assert_eq!(host.auto_remove, Some(true));
    assert!(host.nano_cpus.is_some_and(|nano| nano > 0));
    assert!(host.binds.is_none() && host.cap_add.is_none() && host.cap_drop.is_none());
    assert!(host.port_bindings.is_none() && host.storage_opt.is_none());
    assert!(config.env.is_none() && config.labels.is_none() && config.exposed_ports.is_none());
}

#[test]
fn disk_quota_becomes_a_storage_option() {
    let mut spec = spec();
    spec.disk_mib = Some(2048);
    let host = BollardEngine::container_config(&spec).host_config.unwrap_or_default();
    assert_eq!(
        host.storage_opt,
        Some(HashMap::from([("size".to_string(), "2048m".to_string())]))
    );
}

#[test]
fn overrides_env_labels_and_capabilities_are_forwarded() {
    let mut spec = spec();
    spec.env = vec!["A=1".to_string()];
    spec.binds = vec!["/a:/b".to_string()];
    spec.entrypoint_override = Some(vec![]);
    spec.cmd_override = Some(vec!["sleep".to_string()]);
    spec.working_dir = Some("/w".to_string());
    spec.user = Some("1000".to_string());
    spec.cap_add = vec!["CHOWN".to_string()];
    spec.cap_drop = vec!["ALL".to_string()];
    spec.labels = BTreeMap::from([("k".to_string(), "v".to_string())]);
    let config = BollardEngine::container_config(&spec);
    let host = config.host_config.clone().unwrap_or_default();
    assert_eq!(config.env, Some(vec!["A=1".to_string()]));
    assert_eq!(config.entrypoint, Some(vec![]));
    assert_eq!(config.cmd, Some(vec!["sleep".to_string()]));
    assert_eq!(config.working_dir.as_deref(), Some("/w"));
    assert_eq!(config.user.as_deref(), Some("1000"));
    assert_eq!(config.labels, Some(HashMap::from([("k".to_string(), "v".to_string())])));
    assert_eq!(host.binds, Some(vec!["/a:/b".to_string()]));
    assert_eq!(host.cap_add, Some(vec!["CHOWN".to_string()]));
    assert_eq!(host.cap_drop, Some(vec!["ALL".to_string()]));
}

#[test]
fn secure_hardening_is_forwarded() {
    let mut spec = spec();
    spec.hardening = ContainerHardening::secure();
    spec.hardening.readonly_rootfs = true;
    spec.hardening.runtime = Some("runsc".to_string());
    spec.hardening.tmpfs = vec![("/tmp".to_string(), "rw".to_string())];
    let host = BollardEngine::container_config(&spec).host_config.unwrap_or_default();
    assert_eq!(host.memory_swap, host.memory);
    assert_eq!(host.pids_limit, Some(4096));
    assert_eq!(host.readonly_rootfs, Some(true));
    assert_eq!(host.security_opt, Some(vec!["no-new-privileges".to_string()]));
    assert_eq!(host.runtime.as_deref(), Some("runsc"));
    assert_eq!(host.shm_size, Some(256 * 1024 * 1024));
    assert_eq!(
        host.tmpfs,
        Some(HashMap::from([("/tmp".to_string(), "rw".to_string())]))
    );
}

#[test]
fn exposed_ports_bind_to_loopback_with_an_ephemeral_host_port() {
    let mut spec = spec();
    spec.exposed_ports = vec![8080];
    let config = BollardEngine::container_config(&spec);
    assert_eq!(config.exposed_ports, Some(vec!["8080/tcp".to_string()]));
    let bindings = config
        .host_config
        .and_then(|host| host.port_bindings)
        .unwrap_or_default();
    let binding = bindings.get("8080/tcp").cloned().flatten().unwrap_or_default();
    assert_eq!(binding.len(), 1);
    assert_eq!(binding[0].host_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(binding[0].host_port.as_deref(), Some("0"));
}

#[test]
fn port_endpoints_skip_unbound_ports() {
    let ports: PortMap = HashMap::from([
        (
            "80/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: None,
                host_port: Some("32768".to_string()),
            }]),
        ),
        ("81/tcp".to_string(), None),
        ("82/tcp".to_string(), Some(vec![])),
        (
            "83/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: None,
                host_port: None,
            }]),
        ),
    ]);
    let endpoints = BollardEngine::resolve_port_endpoints(&[80, 81, 82, 83, 84], &ports);
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints.get(&80).map(String::as_str), Some("127.0.0.1:32768"));
}
