use super::errors::RedactionError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPath(String);

impl JsonPath {
    pub fn parse(s: &str) -> Result<Self, RedactionError> {
        if s.is_empty() {
            return Err(RedactionError::InvalidPath("path must not be empty".into()));
        }
        if !s.starts_with('$') {
            return Err(RedactionError::InvalidPath(format!(
                "path must start with $: {s}"
            )));
        }
        Ok(Self(s.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactionAction {
    Mask,
    Hash,
    Drop,
    Replace(String),
    RegexReplace {
        pattern: String,
        replacement: String,
    },
}

impl RedactionAction {
    #[must_use]
    pub fn audit_op(&self) -> &str {
        match self {
            Self::Mask => "mask",
            Self::Hash => "hash",
            Self::Drop => "drop",
            Self::Replace(_) => "mask",
            Self::RegexReplace { .. } => "regex_replace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionRule {
    pub path: JsonPath,
    pub action: RedactionAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_root_and_nested_paths() {
        for path in ["$", "$.foo", "$.foo.bar"] {
            assert!(JsonPath::parse(path).is_ok(), "expected {path} to parse");
        }
    }

    #[test]
    fn parse_rejects_empty_string() {
        assert!(matches!(
            JsonPath::parse(""),
            Err(RedactionError::InvalidPath(_))
        ));
    }

    #[test]
    fn parse_rejects_missing_dollar_prefix() {
        assert!(matches!(
            JsonPath::parse("foo"),
            Err(RedactionError::InvalidPath(_))
        ));
    }
}
