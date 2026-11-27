/// Why a lease capacity was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("worker capacity must be between 1 and {limit} leases")]
pub struct MaxLeasesError {
    /// The most leases a worker may be configured for.
    pub limit: u32,
}
