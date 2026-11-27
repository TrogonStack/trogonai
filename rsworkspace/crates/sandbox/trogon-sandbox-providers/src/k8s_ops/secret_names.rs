use k8s_openapi::api::core::v1::Pod;

use crate::k8s_ops::constants::RESTORE_VOLUME_NAME;

/// Every Secret a cold launch of `job_name` may have created. The names derive
/// from the Job's name, so a worker that never saw the launch, such as one
/// that restarted since, can still find them.
pub fn lease_job_secret_names(job_name: &str) -> Vec<String> {
    vec![
        clone_secret_name(job_name),
        restore_secret_name(job_name),
        s3_volume_secret_name(job_name),
        lease_request_secret_name(job_name),
    ]
}

/// The Secret that keeps the request a lease was created from, so an in-place
/// rollback can rebuild the sandbox after a worker restart. `owner_name` is
/// the lease's Job, or its warm pod. The Secret is owned by that object, so
/// deleting it by any path deletes the Secret too.
pub fn lease_request_secret_name(owner_name: &str) -> String {
    format!("{owner_name}-lease-req")
}

pub fn clone_secret_name(job_name: &str) -> String {
    format!("{job_name}-clone-creds")
}

pub fn restore_secret_name(job_name: &str) -> String {
    format!("{job_name}-restore")
}

pub fn s3_volume_secret_name(job_name: &str) -> String {
    format!("{job_name}-s3-volume")
}

/// The restore Secret a warm pod mounts, read from the pod itself. A warm
/// pod's Secret has a generated name, so after a worker restart the pod spec is
/// the only record of it.
pub fn warm_pod_restore_secret_name(pod: &Pod) -> Option<String> {
    pod.spec
        .as_ref()?
        .volumes
        .as_ref()?
        .iter()
        .find(|volume| volume.name == RESTORE_VOLUME_NAME)?
        .secret
        .as_ref()?
        .secret_name
        .clone()
}
