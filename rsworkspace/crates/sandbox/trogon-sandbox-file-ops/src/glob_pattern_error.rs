/// A glob translates to a regular expression the engine refuses, such as one past the size limit.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("bad glob: {source}")]
pub struct GlobPatternError {
    #[source]
    pub(crate) source: regex::Error,
}
