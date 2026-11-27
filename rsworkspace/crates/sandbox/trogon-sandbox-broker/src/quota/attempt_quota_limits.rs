#[derive(Debug, Clone, Copy)]
pub struct AttemptQuotaLimits {
    pub cpu: u32,
    pub wall: u32,
    pub active: u32,
    /// The broker's configured lease lifetime cap. An attempt can be held
    /// for as long as its lease may live, so the admission window scales
    /// with this rather than with the built-in one-day ceiling.
    pub max_lease_lifetime_seconds: u32,
}
