/// Failures of the Kubernetes ownership fencing and lease recovery.
#[derive(Debug, thiserror::Error)]
pub enum K8sOwnershipError {
    #[error("Kubernetes resource has no confirmed matching namespace")]
    NamespaceNotConfirmed,
    #[error("Kubernetes resource has no confirmed name")]
    NameNotConfirmed,
    #[error("Kubernetes resource has no confirmed UID")]
    UidNotConfirmed,
    #[error("resource inspection returned another name")]
    InspectionNameMismatch,
    #[error("Kubernetes accepted cleanup but the held resource remains")]
    CleanupNotConfirmed,
    #[error("resource was replaced before ownership was attached")]
    ReplacedBeforeOwnership,
    #[error("resource has no revision for ownership fencing")]
    OwnershipRevisionMissing,
    #[error("owned Secret has no name")]
    OwnedSecretNameMissing,
    #[error("stored request belongs to another resource incarnation")]
    StoredRequestForeign,
    #[error("stored request has no UID")]
    StoredRequestUidMissing,
    #[error("stored request has no revision")]
    StoredRequestRevisionMissing,
    #[error("network policy has no name")]
    NetworkPolicyNameMissing,
    #[error("network policy has no matching held ownership receipt")]
    NetworkPolicyNotHeld,
    #[error("network policy has no revision fence")]
    NetworkPolicyRevisionMissing,
    #[error("request owner was replaced before persistence")]
    RequestOwnerReplaced,
    #[error("network policy owner was replaced")]
    NetworkPolicyOwnerReplaced,
    #[error("warm eviction requires an observed idle pod")]
    WarmEvictionNotIdle,
    #[error("idle warm pod has no revision fence")]
    WarmPodRevisionMissing,
    #[error("claimed warm pod has no confirmed restore Secret")]
    RestoreSecretNotConfirmed,
    #[error("warm restore Secret is not owned by the claimed pod UID")]
    RestoreSecretNotOwned,
    #[error("job receipt does not belong to this provider namespace")]
    JobOutsideNamespace,
    #[error("created Kubernetes Job was replaced before its pod became ready")]
    JobReplaced,
    #[error("sandbox pod failed to start: {0}")]
    PodFailedToStart(String),
    #[error("the pod of job {job} did not start within {seconds}s")]
    PodStartTimeout { job: String, seconds: u64 },
    #[error(transparent)]
    Kube(#[from] kube::Error),
}
