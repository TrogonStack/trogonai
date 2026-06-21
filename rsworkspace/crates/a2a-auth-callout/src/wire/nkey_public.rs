use std::fmt;

use nkeys::KeyPair;

use crate::error::AuthCalloutError;

/// NATS NKey public identifier (base32-encoded, prefix `A`/`U`/…).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NkeyPublic(String);

impl NkeyPublic {
    pub fn parse(value: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AuthCalloutError::WireFormat("NKey public key must be non-empty".into()));
        }
        KeyPair::from_public_key(trimmed)
            .map_err(|e| AuthCalloutError::WireFormat(format!("invalid NKey public key: {e}")))?;
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// `verify_jwt_issuer` + the `decode_server_auth_request_jwt` /
// `normalize_auth_request_payload` helpers land alongside the
// `server_auth_request_claims` + `wire_codec` modules in the next slice —
// see `.trogonai/todos/a2a-auth-callout-pr-plan.internal.trogonai.md`.

impl fmt::Debug for NkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NkeyPublic").field(&self.0).finish()
    }
}

impl fmt::Display for NkeyPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_account_public() -> String {
        KeyPair::new_account().public_key()
    }

    #[test]
    fn parse_round_trips_through_as_str_display_debug() {
        let raw = fresh_account_public();
        let np = NkeyPublic::parse(raw.clone()).unwrap();
        assert_eq!(np.as_str(), raw);
        assert_eq!(np.to_string(), raw);
        assert!(format!("{np:?}").starts_with("NkeyPublic"));
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        let raw = fresh_account_public();
        let padded = format!("  {raw}\n");
        assert_eq!(NkeyPublic::parse(padded).unwrap().as_str(), raw);
    }

    #[test]
    fn parse_rejects_empty_input() {
        let e = NkeyPublic::parse("   ").unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }

    #[test]
    fn parse_rejects_garbage() {
        let e = NkeyPublic::parse("not-an-nkey").unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }
}
