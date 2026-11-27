/// Why an endpoint was refused as a SPACE origin.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SpaceOriginError {
    /// A remote endpoint must use HTTPS.
    #[error("remote SPACE endpoints require HTTPS")]
    InsecureRemote,
    /// The URL carries credentials, a query, a fragment or a path.
    #[error("a SPACE endpoint must be a bare origin without credentials, query, fragment or path")]
    NotAnOrigin,
}
