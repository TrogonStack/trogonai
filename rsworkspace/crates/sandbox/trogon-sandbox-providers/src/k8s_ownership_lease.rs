use crate::K8sResourceReceipt;

/// A lease the cluster confirmed this worker still holds, with every object that belongs to it.
pub struct ConfirmedK8sLease {
    pub lease_id: String,
    pub owner: K8sResourceReceipt,
    pub pod: Option<K8sResourceReceipt>,
    pub resources: Vec<K8sResourceReceipt>,
    pub expired: bool,
}
