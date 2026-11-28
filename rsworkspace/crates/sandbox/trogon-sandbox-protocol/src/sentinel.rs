use std::fmt;

use crate::RealValue;
pub use crate::constants::SENTINEL_PREFIX;
use crate::constants::{FAKE_SIGNATURE, PADDING_ALPHABET};
use crate::jwt_structure::is_fake_jwt;

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

    /// Whether `text` carries any sentinel, registered or not: the `fake_value_` prefix every minted sentinel starts
    /// with, or the signature every fake JWT ends with. The comparison ignores ASCII case, because a host name is
    /// lowercased before it is resolved. Use it to keep a value that names a secret's stand-in off a channel the proxy
    /// does not inspect, such as a DNS query.
    pub fn appears_in(text: &str) -> bool {
        [SENTINEL_PREFIX, FAKE_SIGNATURE].iter().any(|marker| {
            text.as_bytes()
                .windows(marker.len())
                .any(|window| window.eq_ignore_ascii_case(marker.as_bytes()))
        })
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
