use std::fmt;

use crate::RealValue;
use crate::jwt_structure::is_fake_jwt;

/// What every sentinel starts with.
pub const SENTINEL_PREFIX: &str = "fake_value_";

const PADDING_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_-";

/// The stand-in a sandboxed process sees: `fake_value_<uuid4>`.
///
/// It is padded with random characters from the same alphabet up to the real value's byte length when the real value
/// is longer. A length-matched sentinel keeps Content-Length invariant under substitution. A shorter real value keeps
/// the full-entropy base sentinel and the proxy re-frames the body instead.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Sentinel(String);

impl Sentinel {
    /// Mints a fresh sentinel for `real`.
    pub fn mint(real: &RealValue) -> Self {
        let mut value = format!("{SENTINEL_PREFIX}{}", uuid::Uuid::new_v4());
        let mut remaining = real.expose().len().saturating_sub(value.len());
        while remaining > 0 {
            for byte in uuid::Uuid::new_v4().as_bytes().iter().take(remaining) {
                value.push(char::from(
                    PADDING_ALPHABET[usize::from(*byte) % PADDING_ALPHABET.len()],
                ));
                remaining -= 1;
            }
        }
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Accepts `raw` as a sentinel only when it has the sentinel alphabet or the shape of a fake JWT.
    pub(crate) fn restore(raw: String) -> Option<Self> {
        let valid = raw.starts_with(SENTINEL_PREFIX)
            && raw
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-');
        (valid || is_fake_jwt(&raw)).then_some(Self(raw))
    }

    /// Wraps a fake token assembled by the JWT masker.
    pub(crate) fn from_fake_jwt(raw: String) -> Self {
        Self(raw)
    }
}

impl fmt::Display for Sentinel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
