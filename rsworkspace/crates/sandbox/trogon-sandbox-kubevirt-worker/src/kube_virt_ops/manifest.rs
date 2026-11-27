use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{Container, PersistentVolumeClaimVolumeSource, Pod, PodSpec, Volume, VolumeMount};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::core::DynamicObject;
use serde_json::{Map, Value};

use super::resources::vm_resource;
use crate::KubeVirtConfig;
use crate::constants::{EXPORT_POD_IMAGE, GUEST_POLICY_LABEL};

/// A JSON object holding `fields` in order.
pub(super) fn object<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// Labels or annotations as a JSON object of strings.
pub(super) fn string_map(values: &BTreeMap<String, String>) -> Map<String, Value> {
    values
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

/// A `VirtualMachine` booting `dv_name` with one masquerade NIC on the pod network that forwards SSH.
pub(super) fn virtual_machine_manifest(
    config: &KubeVirtConfig,
    vm_name: &str,
    dv_name: &str,
    memory_mib: u32,
    vcpu_millis: Option<u32>,
    cloud_init: &str,
    labels: BTreeMap<String, String>,
) -> DynamicObject {
    let mut template_labels = labels.clone();
    template_labels.insert(GUEST_POLICY_LABEL.to_owned(), "true".to_owned());
    let disk = |name: &str| object([("name", name.into()), ("disk", object([("bus", "virtio".into())]))]);
    let interface = object([
        ("name", "default".into()),
        ("masquerade", object([])),
        (
            "ports",
            Value::Array(vec![object([("port", 22.into()), ("protocol", "TCP".into())])]),
        ),
    ]);
    let devices = object([
        ("disks", Value::Array(vec![disk("rootdisk"), disk("cloudinitdisk")])),
        ("interfaces", Value::Array(vec![interface])),
    ]);
    let (resources, cpu) = domain_sizing(memory_mib, vcpu_millis);
    let mut domain = Map::new();
    domain.insert("devices".to_owned(), devices);
    domain.insert("resources".to_owned(), resources);
    if let Some(cpu) = cpu {
        domain.insert("cpu".to_owned(), cpu);
    }
    let volumes = Value::Array(vec![
        object([
            ("name", "rootdisk".into()),
            ("dataVolume", object([("name", dv_name.into())])),
        ]),
        object([
            ("name", "cloudinitdisk".into()),
            ("cloudInitNoCloud", object([("userData", cloud_init.into())])),
        ]),
    ]);
    let template = object([
        (
            "metadata",
            object([("labels", Value::Object(string_map(&template_labels)))]),
        ),
        (
            "spec",
            object([
                ("domain", Value::Object(domain)),
                (
                    "networks",
                    Value::Array(vec![object([("name", "default".into()), ("pod", object([]))])]),
                ),
                ("volumes", volumes),
            ]),
        ),
    ]);
    let spec = object([("running", Value::Bool(true)), ("template", template)]);
    let mut vm = DynamicObject::new(vm_name, &vm_resource())
        .within(&config.namespace)
        .data(object([("spec", spec)]));
    vm.metadata.labels = Some(labels);
    vm
}

/// The cloud-init user data that creates the `sandbox` user with the bootstrap key first, then `extra_keys`.
///
/// The `sudo:` shorthand only appends a sudoers line granting NOPASSWD access; it leaves `Defaults env_reset`
/// alone, which most base images ship enabled and which strips PATH and the environment on `sudo <cmd>`. Sandbox
/// sudo keeps the environment, so a sudoers drop-in disables `env_reset` for the sandbox user. `set_home` stays on,
/// so `sudo <cmd>` still gets `HOME=/root`.
pub(super) fn render_cloud_init_user_data(bootstrap_ssh_pubkey: Option<&str>, extra_keys: &[String]) -> String {
    let keys: Vec<&str> = bootstrap_ssh_pubkey
        .into_iter()
        .chain(extra_keys.iter().map(String::as_str))
        .collect();
    let keys_yaml = if keys.is_empty() {
        "      []".to_owned()
    } else {
        keys.iter()
            .map(|key| format!("      - {key}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let write_files_yaml = [
        "  - path: /etc/sudoers.d/90-sandbox-env",
        "    permissions: '0440'",
        "    content: |",
        "      Defaults:sandbox !env_reset",
    ]
    .join("\n");
    [
        "#cloud-config",
        "users:",
        "- name: sandbox",
        "sudo: ALL=(ALL) NOPASSWD:ALL",
        "shell: /bin/bash",
        "ssh_authorized_keys:",
        &keys_yaml,
        "write_files:",
        &write_files_yaml,
        "runcmd:",
        "- systemctl enable sshd || true",
        "- systemctl start sshd || service ssh start || true",
        "",
    ]
    .join("\n")
}

/// The DataVolume `storage` block: the configured disk size, plus the storage class when one is set.
pub(super) fn storage_spec(config: &KubeVirtConfig) -> Value {
    let mut storage = Map::new();
    storage.insert(
        "resources".to_owned(),
        object([("requests", object([("storage", config.disk_size_quantity().into())]))]),
    );
    if let Some(class) = &config.storage_class {
        storage.insert("storageClassName".to_owned(), class.as_str().into());
    }
    Value::Object(storage)
}

/// The domain's `resources` block and, when the lease names a CPU size, its `cpu` block. The guest sees whole
/// cores, so a fractional request gets the next whole count while the request and limit hold it to the millis
/// asked.
pub(super) fn domain_sizing(memory_mib: u32, vcpu_millis: Option<u32>) -> (Value, Option<Value>) {
    let mut requests = Map::new();
    requests.insert("memory".to_owned(), format!("{memory_mib}Mi").into());
    let Some(millis) = vcpu_millis else {
        return (object([("requests", Value::Object(requests))]), None);
    };
    let cpu = format!("{millis}m");
    requests.insert("cpu".to_owned(), cpu.as_str().into());
    let resources = object([
        ("requests", Value::Object(requests)),
        ("limits", object([("cpu", cpu.into())])),
    ]);
    let cores = millis.div_ceil(1000).max(1);
    (resources, Some(object([("cores", cores.into())])))
}

/// A pod that streams every file of `pvc_name`, mounted read-only, to its log.
pub(super) fn export_pod(name: &str, namespace: &str, pvc_name: &str) -> Pod {
    let container = Container {
        name: "export".to_owned(),
        image: Some(EXPORT_POD_IMAGE.to_owned()),
        command: Some(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "set -e; for f in /disk/*; do cat \"$f\"; done".to_owned(),
        ]),
        volume_mounts: Some(vec![VolumeMount {
            name: "disk".to_owned(),
            mount_path: "/disk".to_owned(),
            read_only: Some(true),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };
    let volume = Volume {
        name: "disk".to_owned(),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: pvc_name.to_owned(),
            read_only: Some(true),
        }),
        ..Volume::default()
    };
    Pod {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            restart_policy: Some("Never".to_owned()),
            containers: vec![container],
            volumes: Some(vec![volume]),
            ..PodSpec::default()
        }),
        ..Pod::default()
    }
}
