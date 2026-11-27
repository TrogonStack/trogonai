use trogon_sandbox_contract::LeaseId;

/// The lease a cancel subject names: its last segment, when it is not empty.
pub fn extract_cancel_lease_id(subject: &str) -> Option<LeaseId> {
    subject
        .split('.')
        .next_back()
        .filter(|segment| !segment.is_empty())
        .map(LeaseId::new)
}
