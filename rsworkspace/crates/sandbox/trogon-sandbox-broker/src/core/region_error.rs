/// Why a lease request's region selection is unusable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum RegionError {
    #[error("fallback_regions requires region")]
    FallbackWithoutRegion,
    #[error("region names must not be empty")]
    EmptyName,
}
