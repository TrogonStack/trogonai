/// Failures of the Kubernetes network policy and egress sidecar builders.
#[derive(Debug, thiserror::Error)]
pub enum K8sNetworkPolicyError {
    #[error("failed to serialize NetworkPolicy for egress proxy: {0}")]
    SerializePolicy(#[source] serde_json::Error),
}
