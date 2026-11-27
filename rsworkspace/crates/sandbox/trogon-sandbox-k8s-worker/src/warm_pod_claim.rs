use trogon_sandbox_providers::K8sResourceReceipt;

#[derive(Debug, Clone)]
pub struct WarmPodClaim {
    pub resources: Vec<K8sResourceReceipt>,
    pub pod_name: String,
    /// The pod's restore Secret, deleted with the lease.
    pub secret_name: Option<String>,
    /// The `NetworkPolicy` applied to the pod for this lease, if the policy
    /// restricts anything.
    pub network_policy_name: Option<String>,
}
