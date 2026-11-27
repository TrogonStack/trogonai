use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;

/// Where a lease's guest runs: the VM a lease holds and the address its VMI reports. The worker's lease state
/// implements it.
#[async_trait]
pub trait GuestLocator: Send + Sync {
    /// The VM this worker runs for `lease_id`, or `None` when it holds no such lease.
    async fn vm_name(&self, lease_id: &LeaseId) -> Option<String>;

    /// The IP address the VMI of `vm_name` reports.
    async fn vmi_ip(&self, vm_name: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
}
