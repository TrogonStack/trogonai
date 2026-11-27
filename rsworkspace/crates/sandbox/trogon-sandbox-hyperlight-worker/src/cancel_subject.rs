use trogon_sandbox_contract::LeaseId;

/// The lease a cancel subject names: its last segment.
pub(crate) fn extract_cancel_lease_id(subject: &str) -> LeaseId {
    LeaseId::new(subject.rsplit('.').next().unwrap_or_default())
}

#[cfg(test)]
mod tests;
