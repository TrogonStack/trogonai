use trogon_sandbox_contract::LeaseId;

/// The lease a cancel subject names: its last segment, or `None` when that segment is empty.
pub(crate) fn extract_cancel_lease_id(subject: &str) -> Option<LeaseId> {
    subject
        .rsplit('.')
        .next()
        .filter(|segment| !segment.is_empty())
        .map(LeaseId::new)
}

#[cfg(test)]
mod tests;
