//! Identities and timestamps the gateway puts on the wire.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Why a gateway identity was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GatewayIdError {
    /// A generation id was not `gen_` followed by 26 Crockford base32 digits.
    #[error("generation id must be `gen_` and 26 unambiguous base32 digits")]
    GenerationId,
    /// A team identifier was empty, too long, or needed percent-encoding.
    #[error("team identifier must be 1 to 128 characters of [A-Za-z0-9_-]")]
    Team,
    /// A timestamp was not a UTC RFC 3339 instant the gateway could have sent.
    #[error("timestamp must be a `Z`-suffixed UTC instant at or after 1970")]
    Timestamp,
    /// A credential held no characters.
    #[error("credential must not be empty")]
    Credential,
}

/// A gateway credential, carried without ever being shown.
///
/// `Debug` reports a length without exposing bytes. There is no `Display` or serde
/// implementation; reading the secret requires naming [`Credential::expose`]. The bytes use
/// `Vec<u8>` so dropping can overwrite them without unsafe code.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential(Vec<u8>);

impl Credential {
    /// Wraps a non-empty credential.
    pub fn new(raw: &str) -> Result<Self, GatewayIdError> {
        if raw.is_empty() {
            return Err(GatewayIdError::Credential);
        }
        Ok(Self(raw.as_bytes().to_vec()))
    }

    /// The empty credential a route that sends no Authorization header holds.
    ///
    /// [`Self::new`] refuses an empty secret because every other route has to
    /// have one. A configured provider declaring `auth: none` has nothing to
    /// send and still names a credential source, so that one case is
    /// constructed here rather than by loosening the constructor.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// The secret itself, for the one caller that has to put it on the wire.
    #[must_use]
    pub fn expose(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Credential({} bytes redacted)", self.0.len())
    }
}

impl Drop for Credential {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// A gateway generation identity.
///
/// The alphabet excludes `I`, `L`, `O`, and `U`, so an id read off a screen and
/// retyped cannot silently become a different valid id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GenerationId(String);

impl GenerationId {
    const PREFIX: &'static str = "gen_";
    const DIGITS: usize = 26;

    /// Parses a generation id.
    pub fn parse(raw: &str) -> Result<Self, GatewayIdError> {
        let digits = raw.strip_prefix(Self::PREFIX).ok_or(GatewayIdError::GenerationId)?;
        if digits.len() != Self::DIGITS || !digits.bytes().all(is_unambiguous_base32) {
            return Err(GatewayIdError::GenerationId);
        }
        Ok(Self(raw.to_string()))
    }

    /// The id as written on the wire.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Uppercase Crockford base32, which drops the four characters that read as each
/// other in the fonts a terminal and a dashboard are likely to use.
const fn is_unambiguous_base32(byte: u8) -> bool {
    matches!(
        byte,
        b'0'..=b'9' | b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'
    )
}

/// A gateway team identifier, in either the `team_` id form or the slug form.
///
/// Team identifiers reach the gateway as a query value, so anything that would
/// need percent-encoding is refused rather than sent as a malformed URL.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TeamId(String);

impl TeamId {
    const MAX_LEN: usize = 128;

    /// Parses a team identifier.
    pub fn parse(raw: &str) -> Result<Self, GatewayIdError> {
        if raw.is_empty() || raw.len() > Self::MAX_LEN {
            return Err(GatewayIdError::Team);
        }
        if !raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(GatewayIdError::Team);
        }
        Ok(Self(raw.to_string()))
    }

    /// The identifier, safe to interpolate into a query string as it stands.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A UTC instant the gateway reported, in milliseconds since the epoch.
///
/// The gateway sends RFC 3339 with a `Z`, and only that: an offset like `+00:00`
/// denotes the same instant but is refused, because accepting one offset form
/// means deciding what to do with the rest, and the gateway never sends them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GatewayTimestamp(i64);

impl GatewayTimestamp {
    /// Parses a `Z`-suffixed UTC instant, keeping milliseconds and truncating
    /// any finer digits the provider sent.
    pub fn parse(text: &str) -> Result<Self, GatewayIdError> {
        let bytes = text.as_bytes();
        if bytes.len() < 20
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || bytes[10] != b'T'
            || bytes[13] != b':'
            || bytes[16] != b':'
        {
            return Err(GatewayIdError::Timestamp);
        }

        let year = digits(&bytes[0..4])?;
        let month = digits(&bytes[5..7])?;
        let day = digits(&bytes[8..10])?;
        let hour = digits(&bytes[11..13])?;
        let minute = digits(&bytes[14..16])?;
        let second = digits(&bytes[17..19])?;
        if year < 1970
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(GatewayIdError::Timestamp);
        }

        let mut cursor = 19;
        let mut fractional_ms: i64 = 0;
        if bytes.get(cursor) == Some(&b'.') {
            cursor += 1;
            let start = cursor;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
            let fraction = &bytes[start..cursor];
            if fraction.is_empty() || fraction.len() > 9 {
                return Err(GatewayIdError::Timestamp);
            }
            let kept = fraction.len().min(3);
            let scale = match kept {
                1 => 100,
                2 => 10,
                _ => 1,
            };
            fractional_ms = i64::from(digits(&fraction[..kept])?) * scale;
        }
        if cursor + 1 != bytes.len() || bytes[cursor] != b'Z' {
            return Err(GatewayIdError::Timestamp);
        }

        let days = days_from_civil(year, month, day);
        let seconds = days
            .checked_mul(86_400)
            .and_then(|whole| whole.checked_add(i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)))
            .ok_or(GatewayIdError::Timestamp)?;
        seconds
            .checked_mul(1000)
            .and_then(|millis| millis.checked_add(fractional_ms))
            .map(Self)
            .ok_or(GatewayIdError::Timestamp)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub fn as_millis(self) -> i64 {
        self.0
    }

    /// Wraps a count the host clock produced, which needs no parsing.
    #[must_use]
    pub fn from_millis(millis: i64) -> Self {
        Self(millis)
    }
}

fn digits(text: &[u8]) -> Result<u32, GatewayIdError> {
    if text.is_empty() {
        return Err(GatewayIdError::Timestamp);
    }
    text.iter().try_fold(0_u32, |result, byte| {
        if !byte.is_ascii_digit() {
            return Err(GatewayIdError::Timestamp);
        }
        result
            .checked_mul(10)
            .and_then(|shifted| shifted.checked_add(u32::from(byte - b'0')))
            .ok_or(GatewayIdError::Timestamp)
    })
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Howard Hinnant's `days_from_civil`, with the era shifted so day zero is
/// 1970-01-01.
fn days_from_civil(year: u32, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2).div_euclid(5) + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era.div_euclid(4) - year_of_era.div_euclid(100) + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

macro_rules! string_value_object {
    ($name:ident) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = GatewayIdError;

            fn try_from(raw: String) -> Result<Self, Self::Error> {
                Self::parse(&raw)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

string_value_object!(GenerationId);
string_value_object!(TeamId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_is_not_printable_and_not_empty() {
        assert_eq!(Credential::new(""), Err(GatewayIdError::Credential));

        let credential = Credential::new("sk-live-secret").expect("non-empty");
        assert_eq!(credential.expose(), "sk-live-secret");

        let shown = format!("{credential:?}");
        assert_eq!(shown, "Credential(14 bytes redacted)");
        assert!(!shown.contains("secret"));
    }

    #[test]
    fn a_generation_id_is_prefixed_and_exactly_sized() {
        let valid = "gen_ABCDEFGHJKMNPQRSTVWXYZ0123";
        assert_eq!(valid.len(), 30);
        assert_eq!(GenerationId::parse(valid).unwrap().as_str(), valid);

        for raw in [
            "",
            "gen_",
            "ABCDEFGHJKMNPQRSTVWXYZ0123",
            "gen_ABCDEFGHJKMNPQRSTVWXYZ012",
            "gen_ABCDEFGHJKMNPQRSTVWXYZ01234",
        ] {
            assert_eq!(GenerationId::parse(raw), Err(GatewayIdError::GenerationId));
        }
    }

    #[test]
    fn the_ambiguous_letters_are_refused() {
        for confusable in ['I', 'L', 'O', 'U'] {
            let raw = format!("gen_{confusable}BCDEFGHJKMNPQRSTVWXYZ0123");
            assert_eq!(
                GenerationId::parse(&raw),
                Err(GatewayIdError::GenerationId),
                "{confusable} reads as another digit"
            );
        }
        assert_eq!(
            GenerationId::parse("gen_abcdefghjkmnpqrstvwxyz0123"),
            Err(GatewayIdError::GenerationId),
            "the alphabet is uppercase"
        );
    }

    #[test]
    fn a_team_identifier_never_needs_percent_encoding() {
        assert_eq!(TeamId::parse("team_abc123").unwrap().as_str(), "team_abc123");
        assert_eq!(TeamId::parse("my-team").unwrap().as_str(), "my-team");
        assert_eq!(TeamId::parse(&"a".repeat(128)).unwrap().as_str().len(), 128);

        for raw in ["", &"a".repeat(129), "my team", "team/other", "team?x=1", "tëam"] {
            assert_eq!(TeamId::parse(raw), Err(GatewayIdError::Team), "{raw}");
        }
    }

    #[test]
    fn a_utc_instant_parses_to_milliseconds() {
        assert_eq!(
            GatewayTimestamp::parse("2026-04-01T12:11:07Z").unwrap().as_millis(),
            1_775_045_467_000
        );
        assert_eq!(
            GatewayTimestamp::parse("2026-04-01T12:11:07.123456Z")
                .unwrap()
                .as_millis(),
            1_775_045_467_123
        );
        assert_eq!(GatewayTimestamp::parse("1970-01-01T00:00:00Z").unwrap().as_millis(), 0);
    }

    #[test]
    fn a_short_fraction_scales_rather_than_truncating() {
        assert_eq!(
            GatewayTimestamp::parse("2026-04-01T12:11:07.5Z").unwrap().as_millis(),
            1_775_045_467_500
        );
        assert_eq!(
            GatewayTimestamp::parse("2026-04-01T12:11:07.05Z").unwrap().as_millis(),
            1_775_045_467_050
        );
    }

    #[test]
    fn a_leap_day_is_accepted_only_in_a_leap_year() {
        assert!(GatewayTimestamp::parse("2024-02-29T00:00:00Z").is_ok());
        assert!(GatewayTimestamp::parse("2000-02-29T00:00:00Z").is_ok());
        assert!(GatewayTimestamp::parse("2026-02-29T00:00:00Z").is_err());
        assert!(GatewayTimestamp::parse("1900-02-29T00:00:00Z").is_err());
    }

    #[test]
    fn anything_other_than_a_utc_instant_is_refused() {
        for raw in [
            "",
            "2026-02-30T12:11:07Z",
            "2026-04-01T12:11:07+00:00",
            "2026-04-01T12:11:07",
            "2026-04-01T12:11:07.Z",
            "2026-04-01T12:11:07.1234567890Z",
            "2026-04-01T24:00:00Z",
            "2026-04-01T12:60:00Z",
            "2026-04-01T12:11:60Z",
            "2026-13-01T12:11:07Z",
            "2026-00-01T12:11:07Z",
            "2026-04-00T12:11:07Z",
            "1969-12-31T23:59:59Z",
            "2026/04/01T12:11:07Z",
            "2026-04-01T12:11:07Zx",
        ] {
            assert!(GatewayTimestamp::parse(raw).is_err(), "{raw} should not parse");
        }
    }

    #[test]
    fn no_mutation_of_a_valid_instant_panics_the_parser() {
        const VALID: &str = "2026-04-01T12:11:07.123456789Z";
        let alphabet = b"0129-:.TZz+x/ \x00\x7f";

        for position in 0..VALID.len() {
            for &byte in alphabet {
                let mut bytes = VALID.as_bytes().to_vec();
                bytes[position] = byte;
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let _ = GatewayTimestamp::parse(text);
                }
            }
        }

        for length in 0..=VALID.len() {
            let _ = GatewayTimestamp::parse(&VALID[..length]);
        }

        for suffix in ["", ".", ".Z", "Z", ".0000000000Z", "\u{85}", "\u{2028}"] {
            let _ = GatewayTimestamp::parse(&format!("2026-04-01T12:11:07{suffix}"));
            let _ = GatewayTimestamp::parse(&format!("9999-12-31T23:59:59{suffix}"));
        }
    }
}
