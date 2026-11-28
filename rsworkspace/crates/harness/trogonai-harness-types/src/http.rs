//! The status an HTTP response came back with.

use std::fmt;

/// An HTTP status code carried by a gateway response.
///
/// A newtype says the same thing without inviting a caller to pass a length, an exit code, or a
/// port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HttpStatus(u16);

impl HttpStatus {
    /// Wraps a status code as it came off the response.
    #[must_use]
    pub const fn new(code: u16) -> Self {
        Self(code)
    }

    /// The code itself.
    #[must_use]
    pub const fn code(self) -> u16 {
        self.0
    }

    /// Whether the response is the plain success a completion arrives on.
    ///
    /// Anything else, including the other 2xx codes, is not a completed turn as
    /// far as this gateway is concerned.
    #[must_use]
    pub const fn is_ok(self) -> bool {
        self.0 == 200
    }

    /// Whether the request was refused rather than failed.
    ///
    /// The distinction is what the headline says: a refusal is about who is
    /// asking, and no amount of retrying changes it.
    #[must_use]
    pub const fn denies_access(self) -> bool {
        matches!(self.0, 401 | 403)
    }
}

impl From<u16> for HttpStatus {
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl fmt::Display for HttpStatus {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_refusals_are_refusals() {
        assert!(HttpStatus::new(401).denies_access());
        assert!(HttpStatus::new(403).denies_access());
        assert!(!HttpStatus::new(402).denies_access());
        assert!(!HttpStatus::new(503).denies_access());
    }
}
