use std::fmt;

/// Name of an MCP tool, as reported in a `tools/list` response.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolNameError {
    #[error("tool name must not be empty")]
    Empty,
}

impl ToolName {
    pub fn new(value: impl Into<String>) -> Result<Self, ToolNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ToolNameError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Number of Unicode scalar values in the name, used by the typosquat
    /// detector's minimum-length threshold.
    pub fn char_len(&self) -> usize {
        self.0.chars().count()
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        assert_eq!(ToolName::new("").unwrap_err(), ToolNameError::Empty);
    }

    #[test]
    fn accepts_non_empty() {
        assert_eq!(ToolName::new("search").unwrap().as_str(), "search");
    }

    #[test]
    fn char_len_counts_unicode_scalars() {
        assert_eq!(ToolName::new("search").unwrap().char_len(), 6);
    }
}
