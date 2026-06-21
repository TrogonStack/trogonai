use std::fmt;

use nkeys::XKey;

use crate::error::AuthCalloutError;

/// NATS XKey (curve25519) public key used for auth-callout payload encryption.
#[derive(Clone, PartialEq, Eq)]
pub struct XkeyPublic(String);

impl XkeyPublic {
    pub fn parse(value: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AuthCalloutError::WireFormat("XKey public key must be non-empty".into()));
        }
        XKey::from_public_key(trimmed)
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid XKey public key: {e}")))?;
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_xkey(&self) -> Result<XKey, AuthCalloutError> {
        XKey::from_public_key(self.as_str())
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid XKey public key: {e}")))
    }
}

impl fmt::Debug for XkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("XkeyPublic").field(&self.0).finish()
    }
}

impl fmt::Display for XkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_xkey_public() -> String {
        XKey::new().public_key()
    }

    #[test]
    fn parse_round_trips_through_as_str_and_display() {
        let raw = fresh_xkey_public();
        let xp = XkeyPublic::parse(raw.clone()).unwrap();
        assert_eq!(xp.as_str(), raw);
        assert_eq!(xp.to_string(), raw);
        assert!(format!("{xp:?}").starts_with("XkeyPublic"));
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        let raw = fresh_xkey_public();
        let padded = format!("  {raw}\n");
        assert_eq!(XkeyPublic::parse(padded).unwrap().as_str(), raw);
    }

    #[test]
    fn parse_rejects_empty_input() {
        let e = XkeyPublic::parse("   ").unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }

    #[test]
    fn parse_rejects_non_xkey_value() {
        let e = XkeyPublic::parse("not-a-key").unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }

    #[test]
    fn to_xkey_yields_usable_keypair() {
        let raw = fresh_xkey_public();
        let xp = XkeyPublic::parse(raw).unwrap();
        xp.to_xkey().unwrap();
    }
}
