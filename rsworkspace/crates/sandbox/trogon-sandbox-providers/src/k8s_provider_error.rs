use crate::{
    K8sNetworkPolicyError, K8sOpsError, K8sOwnershipError, K8sResourceNaming, K8sSecretError, ProxyUidWorkloadError,
    RunAsUserError,
};

/// Failures of the Kubernetes microVM provider.
#[derive(Debug, thiserror::Error)]
pub enum K8sProviderError {
    #[error("host_mount workspaces are not supported on the k8s_microvm provider")]
    HostMountUnsupported,
    #[error(transparent)]
    RunAsUser(#[from] RunAsUserError),
    #[error(
        "privileged: true requires a strong-isolation runtime class (kata/gvisor); resolved runtime class {runtime_class:?} does not look like one"
    )]
    PrivilegedNeedsStrongIsolation { runtime_class: String },
    #[error(transparent)]
    ProxyUidWorkload(#[from] ProxyUidWorkloadError),
    #[error(transparent)]
    Ops(#[from] K8sOpsError),
    #[error(transparent)]
    Ownership(#[from] K8sOwnershipError),
    #[error(transparent)]
    NetworkPolicy(#[from] K8sNetworkPolicyError),
    #[error(transparent)]
    Secret(#[from] K8sSecretError),
    #[error("failed to build Kubernetes client, ensure in-cluster SA token or KUBECONFIG is available")]
    BuildClient(#[source] kube::Error),
    #[error("failed to create egress-proxy ConfigMap")]
    CreateConfigMap(#[source] K8sOpsError),
    #[error("failed to create Kubernetes NetworkPolicy")]
    CreateNetworkPolicy(#[source] K8sOpsError),
    #[error("failed to create Kubernetes NetworkPolicy for the warm pod")]
    CreateWarmPodNetworkPolicy(#[source] K8sOpsError),
    #[error("failed to create Kubernetes Job")]
    CreateJob(#[source] K8sOpsError),
    #[error("failed to patch egress-proxy ConfigMap")]
    PatchConfigMap(#[source] K8sOpsError),
    #[error("failed to patch Kubernetes NetworkPolicy")]
    PatchNetworkPolicy(#[source] K8sOpsError),
    #[error("failed to delete Kubernetes NetworkPolicy for allow-all transition")]
    DeleteNetworkPolicy(#[source] K8sOpsError),
    #[error("serialize network policy to json for ConfigMap patch")]
    SerializeNetworkPolicy(#[source] serde_json::Error),
    #[error("rebuilt NetworkPolicy manifest is missing a spec")]
    NetworkPolicyManifestMissingSpec,
    #[error("sandbox pod {pod} failed to start: {reason}")]
    PodFailedToStart { pod: String, reason: String },
    #[error("the pod of job {job} did not start within {seconds}s")]
    PodStartTimeout { job: String, seconds: u64 },
    #[error("job {job} was still being deleted after {seconds}s")]
    JobDeletionTimeout { job: String, seconds: u64 },
    #[error("{kind} {name} was not found, so it cannot own the lease request")]
    LeaseOwnerNotFound { kind: &'static str, name: String },
    #[error(
        "lease resources use {other} naming; retain the matching resource-naming setting until those resources are drained"
    )]
    ConflictingResourceNaming { other: K8sResourceNaming },
    #[error("warm pod created without a name")]
    WarmPodWithoutName,
    #[error("a warm pod cannot enforce a network policy that needs the egress proxy")]
    WarmPodPolicyNeedsEgressProxy,
    #[error("warm policy has no confirmed pod owner")]
    WarmPolicyWithoutOwner,
    #[error("command exceeded its deadline; termination is unconfirmed for {marker}")]
    TerminationUnconfirmed { marker: String },
    #[error("timeout signal was accepted, but native command completion is unconfirmed for {marker}")]
    CompletionUnconfirmed { marker: String },
}
