//! Generic NATS-safe subject token value object.
//!
//! [`NatsToken<P>`] is parameterized by a [`NatsTokenPolicy`] that encodes the
//! validation flavor (single-token vs. multi-token, ASCII-only vs. UTF-8, etc.).
//! All validation happens at construction; invalid instances are unrepresentable.

use std::marker::PhantomData;
use std::sync::Arc;

use crate::subject_token_violation::SubjectTokenViolation;
use crate::token;

/// Policy trait that controls how a [`NatsToken`] is validated.
///
/// Implemented as associated constants so the compiler can monomorphize away
/// all branching at compile time.
pub trait NatsTokenPolicy {
    /// Whether `.` is accepted as a token separator (multi-token mode).
    const ALLOW_DOTS: bool;

    /// Whether non-ASCII characters are rejected.
    /// When `true`, length is measured in **chars**; otherwise in **bytes**.
    const REQUIRE_ASCII: bool;

    /// Maximum permitted length (chars when `REQUIRE_ASCII`, bytes otherwise).
    const MAX_LENGTH: usize;
}

/// A validated NATS subject token (or dotted token sequence).
///
/// Wraps an `Arc<str>` so cloning is cheap. The policy `P` determines which
/// characters and lengths are accepted.
///
/// Trait impls are hand-written so that `P` does not need to implement
/// `Clone`, `Debug`, `PartialEq`, `Eq`, or `Hash`.
pub struct NatsToken<P: NatsTokenPolicy>(Arc<str>, PhantomData<P>);

impl<P: NatsTokenPolicy> Clone for NatsToken<P> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<P: NatsTokenPolicy> std::fmt::Debug for NatsToken<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("NatsToken").field(&self.0).finish()
    }
}

impl<P: NatsTokenPolicy> PartialEq for NatsToken<P> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<P: NatsTokenPolicy> Eq for NatsToken<P> {}

impl<P: NatsTokenPolicy> std::hash::Hash for NatsToken<P> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<P: NatsTokenPolicy> NatsToken<P> {
    /// Validate and construct a new token.
    pub fn new(s: impl AsRef<str>) -> Result<Self, SubjectTokenViolation> {
        let s = s.as_ref();

        if s.is_empty() {
            return Err(SubjectTokenViolation::Empty);
        }

        if P::REQUIRE_ASCII {
            let mut char_count: usize = 0;
            for ch in s.chars() {
                char_count += 1;
                if char_count > P::MAX_LENGTH {
                    return Err(SubjectTokenViolation::TooLong(char_count));
                }
                if !ch.is_ascii() {
                    return Err(SubjectTokenViolation::InvalidCharacter(ch));
                }
                if ch == '*' || ch == '>' || ch.is_whitespace() {
                    return Err(SubjectTokenViolation::InvalidCharacter(ch));
                }
                if ch == '.' && !P::ALLOW_DOTS {
                    return Err(SubjectTokenViolation::InvalidCharacter(ch));
                }
            }
            if P::ALLOW_DOTS && token::has_consecutive_or_boundary_dots(s) {
                return Err(SubjectTokenViolation::InvalidCharacter('.'));
            }
        } else {
            if let Some(ch) = token::has_wildcards_or_whitespace(s) {
                return Err(SubjectTokenViolation::InvalidCharacter(ch));
            }
            if !P::ALLOW_DOTS {
                if s.contains('.') {
                    return Err(SubjectTokenViolation::InvalidCharacter('.'));
                }
            } else if token::has_consecutive_or_boundary_dots(s) {
                return Err(SubjectTokenViolation::InvalidCharacter('.'));
            }
            if s.len() > P::MAX_LENGTH {
                return Err(SubjectTokenViolation::TooLong(s.len()));
            }
        }

        Ok(Self(s.into(), PhantomData))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<P: NatsTokenPolicy> std::fmt::Display for NatsToken<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<P: NatsTokenPolicy> std::ops::Deref for NatsToken<P> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<P: NatsTokenPolicy> AsRef<str> for NatsToken<P> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SingleTokenPolicy;
    impl NatsTokenPolicy for SingleTokenPolicy {
        const ALLOW_DOTS: bool = false;
        const REQUIRE_ASCII: bool = true;
        const MAX_LENGTH: usize = 128;
    }

    struct MultiTokenPolicy;
    impl NatsTokenPolicy for MultiTokenPolicy {
        const ALLOW_DOTS: bool = true;
        const REQUIRE_ASCII: bool = false;
        const MAX_LENGTH: usize = 128;
    }

    struct AsciiMultiTokenPolicy;
    impl NatsTokenPolicy for AsciiMultiTokenPolicy {
        const ALLOW_DOTS: bool = true;
        const REQUIRE_ASCII: bool = true;
        const MAX_LENGTH: usize = 128;
    }

    type SingleToken = NatsToken<SingleTokenPolicy>;
    type MultiToken = NatsToken<MultiTokenPolicy>;
    type AsciiMultiToken = NatsToken<AsciiMultiTokenPolicy>;

    // ── SingleTokenPolicy (no dots, ASCII-only) ────────────────────────

    #[test]
    fn single_valid() {
        assert!(SingleToken::new("valid-session-123").is_ok());
        assert!(SingleToken::new("a").is_ok());
        assert_eq!(SingleToken::new("hello").unwrap().as_str(), "hello");
    }

    #[test]
    fn single_empty() {
        assert_eq!(SingleToken::new(""), Err(SubjectTokenViolation::Empty));
    }

    #[test]
    fn single_too_long() {
        let long = "a".repeat(129);
        assert_eq!(
            SingleToken::new(&long),
            Err(SubjectTokenViolation::TooLong(129))
        );
        assert!(SingleToken::new("a".repeat(128)).is_ok());
    }

    #[test]
    fn single_rejects_dots() {
        assert_eq!(
            SingleToken::new("a.b"),
            Err(SubjectTokenViolation::InvalidCharacter('.'))
        );
    }

    #[test]
    fn single_rejects_wildcards() {
        assert!(SingleToken::new("a*").is_err());
        assert!(SingleToken::new("a>").is_err());
        assert!(SingleToken::new(">").is_err());
    }

    #[test]
    fn single_rejects_whitespace() {
        assert!(SingleToken::new("a b").is_err());
        assert!(SingleToken::new("a\t").is_err());
        assert!(SingleToken::new("a\n").is_err());
    }

    #[test]
    fn single_rejects_non_ascii() {
        assert_eq!(
            SingleToken::new("séssion"),
            Err(SubjectTokenViolation::InvalidCharacter('é'))
        );
    }

    // ── MultiTokenPolicy (dots ok, byte-length) ───────────────────────

    #[test]
    fn multi_valid_simple() {
        assert!(MultiToken::new("acp").is_ok());
        assert!(MultiToken::new("a").is_ok());
    }

    #[test]
    fn multi_valid_dotted() {
        assert_eq!(
            MultiToken::new("my.multi.part").unwrap().as_str(),
            "my.multi.part"
        );
        assert!(MultiToken::new("a.b").is_ok());
        assert!(MultiToken::new("vendor.operation").is_ok());
    }

    #[test]
    fn multi_empty() {
        assert_eq!(MultiToken::new(""), Err(SubjectTokenViolation::Empty));
    }

    #[test]
    fn multi_too_long() {
        let long = "a".repeat(129);
        assert_eq!(
            MultiToken::new(&long),
            Err(SubjectTokenViolation::TooLong(129))
        );
        assert!(MultiToken::new("a".repeat(128)).is_ok());
    }

    #[test]
    fn multi_rejects_wildcards() {
        assert!(MultiToken::new("acp.*").is_err());
        assert!(MultiToken::new("acp.>").is_err());
    }

    #[test]
    fn multi_rejects_whitespace() {
        assert!(MultiToken::new("acp prefix").is_err());
        assert!(MultiToken::new("acp\t").is_err());
        assert!(MultiToken::new("acp\n").is_err());
    }

    #[test]
    fn multi_rejects_malformed_dots() {
        assert!(MultiToken::new("..method").is_err());
        assert!(MultiToken::new("method..name").is_err());
        assert!(MultiToken::new(".method").is_err());
        assert!(MultiToken::new("method.").is_err());
        assert!(MultiToken::new(".").is_err());
        assert!(MultiToken::new("acp..foo").is_err());
        assert!(MultiToken::new(".acp").is_err());
        assert!(MultiToken::new("acp.").is_err());
    }

    #[test]
    fn multi_accepts_non_ascii() {
        assert!(MultiToken::new("préfixe").is_ok());
    }

    // ── Trait impls ────────────────────────────────────────────────────

    #[test]
    fn display_and_deref() {
        let t = SingleToken::new("my-session").unwrap();
        assert_eq!(format!("{}", t), "my-session");
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("my"));
    }

    #[test]
    fn as_ref_str() {
        let t = MultiToken::new("hello").unwrap();
        let s: &str = t.as_ref();
        assert_eq!(s, "hello");
    }

    #[test]
    fn clone_and_eq() {
        let a = SingleToken::new("abc").unwrap();
        let b = a.clone();
        assert_eq!(a, b);
    }

    // ── AsciiMultiTokenPolicy (dots ok, ASCII-only) ─────────────────────

    #[test]
    fn ascii_multi_valid_dotted() {
        assert_eq!(
            AsciiMultiToken::new("my.multi.part").unwrap().as_str(),
            "my.multi.part"
        );
        assert!(AsciiMultiToken::new("a.b").is_ok());
        assert!(AsciiMultiToken::new("vendor.operation").is_ok());
    }

    #[test]
    fn ascii_multi_valid_simple() {
        assert!(AsciiMultiToken::new("simple").is_ok());
        assert!(AsciiMultiToken::new("a").is_ok());
    }

    #[test]
    fn ascii_multi_rejects_non_ascii() {
        assert_eq!(
            AsciiMultiToken::new("préfixe"),
            Err(SubjectTokenViolation::InvalidCharacter('é'))
        );
    }

    #[test]
    fn ascii_multi_rejects_malformed_dots() {
        assert!(AsciiMultiToken::new("..method").is_err());
        assert!(AsciiMultiToken::new("method..name").is_err());
        assert!(AsciiMultiToken::new(".method").is_err());
        assert!(AsciiMultiToken::new("method.").is_err());
        assert!(AsciiMultiToken::new(".").is_err());
    }

    #[test]
    fn ascii_multi_rejects_wildcards() {
        assert!(AsciiMultiToken::new("a.*").is_err());
        assert!(AsciiMultiToken::new("a.>").is_err());
    }

    #[test]
    fn ascii_multi_rejects_whitespace() {
        assert!(AsciiMultiToken::new("a b").is_err());
        assert!(AsciiMultiToken::new("a\t").is_err());
    }
}
