use trogon_sandbox_contract::LeaseId;

/// The subject a lease's lifecycle events are published on.
pub fn lifecycle_subject(lease_id: &LeaseId) -> String {
    format!("sandbox.evidence.{}.lifecycle", lease_id.as_str())
}
