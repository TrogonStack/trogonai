/// What the cluster shows about a warm claim whose outcome was not observed directly.
pub(super) enum ClaimResolution {
    /// The claim took effect, so the lease owns the VM and its disk.
    Owned,
    /// Another writer changed the VM first, so the claim never took effect.
    NotOwned,
    /// The VM and its original disk are gone, so nothing is left to own.
    Settled,
}
