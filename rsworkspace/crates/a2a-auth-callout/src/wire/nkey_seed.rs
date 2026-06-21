use std::fmt;

use nkeys::{KeyPair, XKey};

use crate::error::AuthCalloutError;

/// NATS NKey or XKey seed (sensitive; load from env/file at process edge only).
#[derive(Clone)]
pub struct NkeySeed(String);

impl NkeySeed {
    pub fn parse(seed: impl Into<String>) -> Result<Self, AuthCalloutError> {
        let seed = seed.into();
        let trimmed = seed.trim();
        if trimmed.is_empty() {
            return Err(AuthCalloutError::WireFormat("NKey seed must be non-empty".into()));
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_signing_keypair(&self) -> Result<KeyPair, AuthCalloutError> {
        KeyPair::from_seed(self.as_str()).map_err(|e| AuthCalloutError::WireFormat(format!("invalid NKey seed: {e}")))
    }

    pub fn to_xkey(&self) -> Result<XKey, AuthCalloutError> {
        XKey::from_seed(self.as_str()).map_err(|e| AuthCalloutError::WireFormat(format!("invalid XKey seed: {e}")))
    }
}

impl fmt::Debug for NkeySeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NkeySeed([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_nkey_seed() -> String {
        // Account seed; works for to_signing_keypair but not for to_xkey
        // (that requires a curve25519 seed).
        KeyPair::new_account().seed().unwrap()
    }

    fn fresh_xkey_seed() -> String {
        XKey::new().seed().unwrap()
    }

    #[test]
    fn parse_trims_and_round_trips() {
        let raw = fresh_nkey_seed();
        let padded = format!("\n  {raw}  ");
        let s = NkeySeed::parse(padded).unwrap();
        assert_eq!(s.as_str(), raw);
    }

    #[test]
    fn parse_rejects_empty() {
        let e = NkeySeed::parse("   ").unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }

    #[test]
    fn debug_redacts_seed() {
        let s = NkeySeed::parse(fresh_nkey_seed()).unwrap();
        assert_eq!(format!("{s:?}"), "NkeySeed([redacted])");
    }

    #[test]
    fn to_signing_keypair_round_trip() {
        let raw = fresh_nkey_seed();
        let s = NkeySeed::parse(raw.clone()).unwrap();
        let kp = s.to_signing_keypair().unwrap();
        assert!(kp.public_key().starts_with('A'));
    }

    #[test]
    fn to_signing_keypair_rejects_garbage() {
        let s = NkeySeed::parse("SUAGARBAGE").unwrap();
        let e = s.to_signing_keypair().unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }

    #[test]
    fn to_xkey_round_trip() {
        let raw = fresh_xkey_seed();
        let s = NkeySeed::parse(raw).unwrap();
        s.to_xkey().unwrap();
    }

    #[test]
    fn to_xkey_rejects_garbage() {
        let s = NkeySeed::parse("SXGARBAGE").unwrap();
        let e = s.to_xkey().unwrap_err();
        assert!(matches!(e, AuthCalloutError::WireFormat(_)));
    }
}
