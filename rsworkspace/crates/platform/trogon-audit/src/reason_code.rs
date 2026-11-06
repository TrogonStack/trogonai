use crate::constants::MAX_REASON_CODE_BYTES;

/// A closed reason code owned by the surface (ADR#0086 Decision 5).
///
/// It is built from a `&'static str`, so only a literal the surface compiled in can
/// become one. A message echoed from a request cannot, which is how the A2A
/// `AuditOutcome::Err { code, message }` keeps its code and drops its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReasonCode(&'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReasonCodeError {
    #[error("reason code is empty")]
    Empty,
    #[error("reason code is {0} bytes, over the {MAX_REASON_CODE_BYTES}-byte limit")]
    TooLong(usize),
    #[error("reason code must be lower_snake and start with a letter")]
    NotLowerSnake,
}

impl ReasonCode {
    pub fn new(code: &'static str) -> Result<Self, ReasonCodeError> {
        if code.is_empty() {
            return Err(ReasonCodeError::Empty);
        }
        if code.len() > MAX_REASON_CODE_BYTES {
            return Err(ReasonCodeError::TooLong(code.len()));
        }
        let starts_with_letter = code.starts_with(|c: char| c.is_ascii_lowercase());
        let only_snake = code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !(starts_with_letter && only_snake) {
            return Err(ReasonCodeError::NotLowerSnake);
        }
        Ok(Self(code))
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[cfg(test)]
mod tests;
