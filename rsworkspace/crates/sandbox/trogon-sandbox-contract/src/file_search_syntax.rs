use serde::{Deserialize, Serialize};

/// Whether a search pattern is a literal string or a regular expression.
///
/// It serializes as the JSON boolean `regex`, `true` for `Regex`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum FileSearchSyntax {
    #[default]
    Literal,
    Regex,
}

impl FileSearchSyntax {
    pub const fn is_regex(self) -> bool {
        matches!(self, Self::Regex)
    }
}

impl From<bool> for FileSearchSyntax {
    fn from(value: bool) -> Self {
        if value { Self::Regex } else { Self::Literal }
    }
}

impl From<FileSearchSyntax> for bool {
    fn from(value: FileSearchSyntax) -> Self {
        value.is_regex()
    }
}
