//! NATS subject token validation (wasm-clean subset of `trogon-nats`).

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SubjectTokenViolation {
    #[error("subject token is empty")]
    Empty,
    #[error("subject token contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("subject token is too long ({0} bytes, max 128)")]
    TooLong(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DottedNatsToken(String);

impl DottedNatsToken {
    pub fn new(s: impl AsRef<str>) -> Result<Self, SubjectTokenViolation> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(SubjectTokenViolation::Empty);
        }
        if s.contains("..") || s.starts_with('.') || s.ends_with('.') {
            return Err(SubjectTokenViolation::InvalidCharacter('.'));
        }
        for ch in s.chars() {
            if ch.is_control() || ch.is_whitespace() {
                return Err(SubjectTokenViolation::InvalidCharacter(ch));
            }
            if matches!(ch, '*' | '>' | '\0') {
                return Err(SubjectTokenViolation::InvalidCharacter(ch));
            }
        }
        if s.len() > 128 {
            return Err(SubjectTokenViolation::TooLong(s.len()));
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DottedNatsToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
