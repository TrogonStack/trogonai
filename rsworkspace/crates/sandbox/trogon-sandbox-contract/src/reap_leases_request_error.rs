/// A reap request that [`ReapLeasesRequest`](crate::ReapLeasesRequest) refuses to build.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ReapLeasesRequestError {
    #[error("a reap request needs at least one of tags, older_than_seconds, or idle_for_seconds")]
    NoFilter,
}
