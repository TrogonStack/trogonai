use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RuleName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleNameError {
    #[error("rule name must not be empty or whitespace-only")]
    Empty,
}

impl RuleName {
    /// Construct a validated rule name. Rejects empty / whitespace-only
    /// names so an invalid `RuleName` value can't exist in memory.
    /// Use `try_new` from new code; the legacy `new` is kept for the
    /// loader path that already filters non-empty file stems.
    pub fn try_new(name: impl Into<String>) -> Result<Self, RuleNameError> {
        let raw = name.into();
        if raw.trim().is_empty() {
            return Err(RuleNameError::Empty);
        }
        Ok(Self(raw))
    }

    /// Infallible constructor used by the loader and the
    /// `evaluation_error` sentinel — the inputs are already known to be
    /// non-empty (file stems / static string literals). Defer to
    /// [`Self::try_new`] when accepting an arbitrary caller string.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn evaluation_error() -> Self {
        Self::new("evaluation_error")
    }
}

impl fmt::Display for RuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
