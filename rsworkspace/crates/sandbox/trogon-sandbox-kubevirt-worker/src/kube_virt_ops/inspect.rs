use std::collections::BTreeMap;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::core::DynamicObject;
use serde_json::Value;

use crate::constants::{LEGACY_WARM_MEMORY_MIB, WARM_MEMORY_ANNOTATION};

/// The string at `status.{field}` of an object's data.
pub(super) fn status_str<'a>(data: &'a Value, field: &str) -> Option<&'a str> {
    data.get("status")?.get(field)?.as_str()
}

/// Whether `status.conditions` holds a condition of `kind` whose status is `True`.
pub(super) fn has_condition(data: &Value, kind: &str) -> bool {
    data.get("status")
        .and_then(|status| status.get("conditions"))
        .and_then(Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition.get("type").and_then(Value::as_str) == Some(kind)
                    && condition.get("status").and_then(Value::as_str) == Some("True")
            })
        })
}

/// Whether a `VirtualMachine` with `uid` owns `object`.
pub(super) fn owned_by(object: &DynamicObject, uid: &str) -> bool {
    object.metadata.owner_references.as_ref().is_some_and(|owners| {
        owners
            .iter()
            .any(|owner| owner.kind == "VirtualMachine" && owner.uid == uid)
    })
}

/// The value of label `key`.
pub(super) fn label<'a>(metadata: &'a ObjectMeta, key: &str) -> Option<&'a str> {
    metadata.labels.as_ref()?.get(key).map(String::as_str)
}

/// The value of annotation `key`.
pub(super) fn annotation<'a>(metadata: &'a ObjectMeta, key: &str) -> Option<&'a str> {
    metadata.annotations.as_ref()?.get(key).map(String::as_str)
}

/// The PVC a VM snapshot content backs its first volume up to.
pub(super) fn pvc_from_snapshot_content(item: &DynamicObject) -> Option<String> {
    item.data
        .get("status")?
        .get("volumeBackups")?
        .as_array()?
        .first()?
        .get("persistentVolumeClaimName")?
        .as_str()
        .map(str::to_owned)
}

/// A warm VM fits a lease asking for no more memory than it booted with. An unannotated VM counts as
/// [`LEGACY_WARM_MEMORY_MIB`] so it stays claimable instead of idling until the sweep; an unparseable annotation
/// never fits.
pub(super) fn warm_vm_has_memory(annotations: &BTreeMap<String, String>, memory_mib: u32) -> bool {
    let warm = annotations
        .get(WARM_MEMORY_ANNOTATION)
        .map_or(Some(LEGACY_WARM_MEMORY_MIB), |value| value.parse::<u32>().ok());
    warm.is_some_and(|warm| warm >= memory_mib)
}

/// Why a VM's stored NIC definition would let the guest escape the pod's network policy.
///
/// Anti-spoofing for KubeVirt is the CNI's job: the pod's NetworkPolicy and the CNI's own source checks see the
/// pod's veth, not the guest. That only holds while every guest NIC is a masquerade interface on the pod network,
/// so KubeVirt NATs the guest's frames to the pod address. A `bridge`, `sriov` or `passt` binding, a secondary
/// network or host networking would hand the guest's own MAC and IP to the fabric.
pub(super) fn nic_binding_findings(vm: &Value) -> Vec<String> {
    let spec = &vm["spec"]["template"]["spec"];
    let mut findings = Vec::new();
    let interfaces = spec["domain"]["devices"]["interfaces"]
        .as_array()
        .filter(|interfaces| !interfaces.is_empty());
    let Some(interfaces) = interfaces else {
        findings.push("no interfaces are defined".to_owned());
        return network_findings(spec, findings);
    };
    for interface in interfaces {
        let name = interface["name"].as_str().unwrap_or("?");
        if interface.get("masquerade").is_none() {
            findings.push(format!("interface `{name}` is not a masquerade binding"));
        }
        let bindings = ["bridge", "sriov", "passt", "slirp", "macvtap"];
        for other in bindings.into_iter().filter(|other| interface.get(other).is_some()) {
            findings.push(format!("interface `{name}` uses the `{other}` binding"));
        }
    }
    network_findings(spec, findings)
}

fn network_findings(spec: &Value, mut findings: Vec<String>) -> Vec<String> {
    let networks = spec["networks"].as_array().filter(|networks| !networks.is_empty());
    let Some(networks) = networks else {
        findings.push("no networks are defined".to_owned());
        return findings;
    };
    for network in networks.iter().filter(|network| network.get("pod").is_none()) {
        let name = network["name"].as_str().unwrap_or("?");
        findings.push(format!("network `{name}` is not the pod network"));
    }
    findings
}

/// The first non-empty address in a VMI's `status.interfaces`.
pub(super) fn first_interface_ip(vmi: &Value) -> Option<String> {
    vmi.get("status")?
        .get("interfaces")?
        .as_array()?
        .iter()
        .find_map(|interface| {
            interface
                .get("ipAddress")
                .and_then(Value::as_str)
                .filter(|ip| !ip.is_empty())
        })
        .map(str::to_owned)
}
