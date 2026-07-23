use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RevisionNumber(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RevisionNumberError {
    #[error("revision number must be at least 1")]
    Zero,
}

impl RevisionNumber {
    pub const GENESIS: Self = Self(1);

    pub fn new(value: u64) -> Result<Self, RevisionNumberError> {
        if value == 0 {
            return Err(RevisionNumberError::Zero);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RevisionNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests;
