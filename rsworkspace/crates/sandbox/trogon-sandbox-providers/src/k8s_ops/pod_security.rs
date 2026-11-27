use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{EmptyDirVolumeSource, PodSecurityContext, SeccompProfile, Volume};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::{DeleteParams, PropagationPolicy};

use crate::k8s_ops::constants::{
    KEEP_ALIVE_SCRIPT, RUNTIME_TMP_MAX_MIB, RUNTIME_TMP_VOLUME, SECCOMP_PROFILE_TYPE, WORKSPACE_FS_GROUP,
};

/// Pod-level hardening applied to every sandbox pod.
pub(crate) fn hardened_pod_security_context() -> PodSecurityContext {
    PodSecurityContext {
        seccomp_profile: Some(SeccompProfile {
            type_: SECCOMP_PROFILE_TYPE.to_string(),
            ..Default::default()
        }),
        fs_group: Some(WORKSPACE_FS_GROUP),
        ..Default::default()
    }
}

/// Deletes a Job together with its pods. A Job deleted through the API with no
/// propagation policy orphans its pods, so the sandbox would keep running
/// after its lease was released.
pub(crate) fn job_delete_params() -> DeleteParams {
    DeleteParams {
        grace_period_seconds: Some(0),
        propagation_policy: Some(PropagationPolicy::Background),
        ..Default::default()
    }
}

pub(crate) fn keep_alive_command() -> Vec<String> {
    vec!["/bin/sh".to_string(), "-c".to_string(), KEEP_ALIVE_SCRIPT.to_string()]
}

/// CPU and memory quantities for a sandbox container. A zero memory size means
/// the lease named none, so the container gets no memory quantity rather than
/// a `0Mi` limit the kubelet would kill it for exceeding.
pub(crate) fn container_resources(cpu: f32, memory_mib: u32) -> BTreeMap<String, Quantity> {
    let mut resources = BTreeMap::new();
    resources.insert("cpu".to_string(), Quantity(cpu.to_string()));
    if memory_mib > 0 {
        resources.insert("memory".to_string(), Quantity(format!("{memory_mib}Mi")));
    }
    resources
}

pub(crate) fn runtime_tmp_volume(memory_mib: u32) -> Volume {
    let size_mib = if memory_mib == 0 {
        RUNTIME_TMP_MAX_MIB
    } else {
        memory_mib.min(RUNTIME_TMP_MAX_MIB)
    };
    Volume {
        name: RUNTIME_TMP_VOLUME.into(),
        empty_dir: Some(EmptyDirVolumeSource {
            medium: Some("Memory".into()),
            size_limit: Some(Quantity(format!("{size_mib}Mi"))),
        }),
        ..Default::default()
    }
}
