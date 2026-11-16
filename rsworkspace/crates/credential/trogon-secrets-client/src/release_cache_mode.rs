/// Whether a client may hold releases right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseCacheMode {
    /// The invalidation reader is in place, so a held copy hears its notice.
    Caching,
    /// The invalidation reader lost its place: nothing is held, and every
    /// release is asked for again until it reads again.
    Uncached,
}
