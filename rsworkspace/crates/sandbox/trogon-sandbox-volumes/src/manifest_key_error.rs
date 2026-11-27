/// A string or path is not a valid relative object key.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ManifestKeyError {
    #[error("manifest key must not be empty")]
    Empty,
    #[error("manifest key must not contain an empty segment")]
    EmptySegment,
    #[error("manifest key segment '{0}' is a relative path component")]
    RelativeSegment(String),
    #[error("manifest key must not contain a backslash")]
    Backslash,
    #[error("manifest key path must be relative and made of plain file names")]
    NotRelative,
    #[error("manifest key path must be valid UTF-8")]
    NonUnicode,
}
