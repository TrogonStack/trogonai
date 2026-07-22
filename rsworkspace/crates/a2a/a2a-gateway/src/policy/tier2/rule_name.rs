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
    ///
    /// This is the only public constructor. Internal call sites that
    /// already know the input is valid (file stems pre-filtered by the
    /// loader, static literals like the `evaluation_error` sentinel)
    /// use [`Self::new_unchecked`], which is crate-private so external
    /// callers can't bypass the validation contract.
    pub fn new(name: impl Into<String>) -> Result<Self, RuleNameError> {
        let raw = name.into();
        if raw.trim().is_empty() {
            return Err(RuleNameError::Empty);
        }
        Ok(Self(raw))
    }

    /// Crate-private infallible constructor. Use only when the caller
    /// has already enforced non-emptiness — `pub(crate)` keeps external
    /// callers on the validating [`Self::new`] path.
    pub(crate) fn new_unchecked(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn evaluation_error() -> Self {
        // Static literal — known non-empty.
        Self::new_unchecked("evaluation_error")
    }
}

impl fmt::Display for RuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
