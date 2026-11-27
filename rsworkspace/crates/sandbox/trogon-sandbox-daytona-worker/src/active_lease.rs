/// A sandbox this worker holds for a lease.
pub(crate) struct ActiveLease {
    pub(crate) id: String,
    pub(crate) region: Option<String>,
}
