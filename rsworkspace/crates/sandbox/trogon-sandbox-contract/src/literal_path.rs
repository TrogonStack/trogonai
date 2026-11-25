use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::LiteralPathError;

/// An absolute path inside the sandbox, canonical by lexical rules only.
///
/// `.` segments and duplicate slashes collapse, `..` folds into its parent, the trailing slash is dropped, and a
/// `..` above the root is refused. The filesystem is never consulted, so symlinks are not resolved here. A path
/// holds at most [`LiteralPath::MAX_LEN`] bytes and no NUL byte.
///
/// Every character is a plain character of a name, `*` and `[` included, and a backslash is not a separator:
/// `/a\b` is one name, distinct from `/a/b`. It serializes as a JSON string, and construction, parsing, and
/// deserialization all enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LiteralPath(String);

impl LiteralPath {
    /// Longest path the wire accepts, in bytes.
    pub const MAX_LEN: usize = 4096;

    pub fn new(value: impl Into<String>) -> Result<Self, LiteralPathError> {
        let raw = value.into();
        if raw.is_empty() {
            return Err(LiteralPathError::Empty);
        }
        if raw.contains('\0') {
            return Err(LiteralPathError::ContainsNul);
        }
        if raw.len() > Self::MAX_LEN {
            return Err(LiteralPathError::TooLong { max: Self::MAX_LEN });
        }
        if !raw.starts_with('/') {
            return Err(LiteralPathError::NotAbsolute(raw));
        }
        let mut segments: Vec<&str> = Vec::new();
        for segment in raw.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.pop().is_none() {
                        return Err(LiteralPathError::EscapesRoot(raw.clone()));
                    }
                }
                other => segments.push(other),
            }
        }
        Ok(Self(format!("/{}", segments.join("/"))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LiteralPath {
    type Error = LiteralPathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for LiteralPath {
    type Err = LiteralPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<LiteralPath> for String {
    fn from(path: LiteralPath) -> Self {
        path.0
    }
}

impl fmt::Display for LiteralPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
