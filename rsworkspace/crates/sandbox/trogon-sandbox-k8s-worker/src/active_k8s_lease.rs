use std::sync::Arc;

use trogon_sandbox_contract::LeaseRequest;
use trogon_sandbox_providers::K8sResourceReceipt;

#[derive(Clone)]
pub(crate) struct ActiveK8sLease {
    pub(crate) owner: K8sResourceReceipt,
    pub(crate) resources: Vec<K8sResourceReceipt>,
    /// The warm pod's name, or the Job's name for a cold launch. Names the
    /// lease's Kubernetes objects.
    pub(crate) sandbox_id: String,
    /// The pod that exec runs in. Equal to `sandbox_id` for a warm pod; the
    /// Job's pod (which has a generated suffix) for a cold launch.
    pub(crate) pod_name: String,
    pub(crate) is_warm_pod: bool,
    /// The request this lease was created from, with later in-place changes
    /// such as a network policy update applied, so a rollback rebuilds the
    /// sandbox as it is now. `None` for a lease recovered with no stored copy.
    pub(crate) request: Option<Arc<LeaseRequest>>,
    /// Whether a copy of `request` lives in a Secret in the cluster, which
    /// then has to follow every change to `request`.
    pub(crate) request_persisted: bool,
}
