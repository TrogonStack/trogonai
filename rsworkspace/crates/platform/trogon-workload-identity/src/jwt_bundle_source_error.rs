/// Why [`crate::JwtBundleSource::current`] has no set to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JwtBundleSourceError {
    #[error("no JWT bundle set has been delivered yet")]
    NeverDelivered,
    #[error("the JWT bundle stream has been lost for longer than the staleness bound")]
    Stale,
}
