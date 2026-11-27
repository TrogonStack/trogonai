#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum NamingError {
    /// The tenant id is not usable as a DNS-1123 label as-is.
    #[error("tenant {tenant:?} cannot be used as a substrate atespace: {reason}")]
    TenantNotADnsLabel { tenant: String, reason: String },
    /// The lease id contains something that cannot appear in a DNS-1123 label.
    #[error("lease id {lease_id:?} cannot be used as a substrate actor name: {reason}")]
    LeaseIdNotALabel { lease_id: String, reason: String },
}
