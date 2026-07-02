/// Typed failure reasons the OSV-backed feed gate surfaces before it is
/// collapsed into a fail-closed [`crate::FeedVerdict`]. Kept distinct from
/// `FeedVerdict` so callers that want the raw failure (e.g. for logging)
/// are not forced to pattern-match through the verdict enum.
#[derive(Debug, thiserror::Error)]
pub enum CveFeedError {
    #[error("OSV feed request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("OSV feed returned malformed response: {0}")]
    MalformedResponse(#[from] serde_json::Error),
}
