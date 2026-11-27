//! Signed, expiring preview tokens.
//!
//! A token binds `tenant`, `lease_id`, `port` and an expiry under an HMAC-SHA256 keyed by one of the configured
//! signing keys, selected by key id so keys rotate without invalidating live tokens.

use std::collections::BTreeMap;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use trogon_sandbox_contract::LeaseId;

use crate::constants::{MAX_PREVIEW_KEY_ID_LEN, MIN_PREVIEW_KEY_BYTES, PREVIEW_MAC_CONTEXT, PREVIEW_TOKEN_VERSION};
use crate::{PreviewClaims, PreviewConfigError, PreviewTokenError};

type HmacSha256 = Hmac<Sha256>;

/// Signing keys addressed by key id. Never prints key material.
pub struct PreviewKeys {
    active: String,
    keys: BTreeMap<String, Vec<u8>>,
}

impl fmt::Debug for PreviewKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreviewKeys")
            .field("active", &self.active)
            .field("key_ids", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn valid_key_id(kid: &str) -> bool {
    !kid.is_empty()
        && kid.len() <= MAX_PREVIEW_KEY_ID_LEN
        && kid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn mac(key: &[u8], signed: &str) -> Result<HmacSha256, PreviewTokenError> {
    let mut mac = HmacSha256::new_from_slice(key)?;
    mac.update(PREVIEW_MAC_CONTEXT);
    mac.update(signed.as_bytes());
    Ok(mac)
}

impl PreviewKeys {
    /// Parses `kid=hexkey[,kid=hexkey...]`. `active` selects the key that signs new tokens and defaults to the first
    /// entry. Every listed key still verifies, which is how rotation overlaps.
    pub fn parse(spec: &str, active: Option<&str>) -> Result<Self, PreviewConfigError> {
        let mut keys = BTreeMap::new();
        let mut first: Option<String> = None;
        for entry in spec.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
            let (kid, hex_key) = entry.split_once('=').ok_or(PreviewConfigError::MissingSeparator)?;
            if !valid_key_id(kid) {
                return Err(PreviewConfigError::InvalidKeyId { kid: kid.into() });
            }
            let bytes = hex::decode(hex_key.trim()).map_err(|_| PreviewConfigError::InvalidHex { kid: kid.into() })?;
            if bytes.len() < MIN_PREVIEW_KEY_BYTES {
                return Err(PreviewConfigError::KeyTooShort { kid: kid.into() });
            }
            if keys.insert(kid.to_string(), bytes).is_some() {
                return Err(PreviewConfigError::DuplicateKeyId { kid: kid.into() });
            }
            first.get_or_insert_with(|| kid.to_string());
        }
        let first = first.ok_or(PreviewConfigError::Empty)?;
        let active = match active.map(str::trim).filter(|active| !active.is_empty()) {
            Some(kid) if keys.contains_key(kid) => kid.to_string(),
            Some(kid) => return Err(PreviewConfigError::UnknownActiveKey { kid: kid.into() }),
            None => first,
        };
        Ok(Self { active, keys })
    }

    /// Signs `claims` with the active key.
    pub fn issue(&self, claims: &PreviewClaims) -> Result<String, PreviewTokenError> {
        let payload = serde_json::to_vec(claims).map_err(PreviewTokenError::Encode)?;
        let signed = format!(
            "{PREVIEW_TOKEN_VERSION}.{}.{}",
            self.active,
            URL_SAFE_NO_PAD.encode(payload)
        );
        let key = self.keys.get(&self.active).ok_or(PreviewTokenError::UnknownKey)?;
        let tag = mac(key, &signed)?.finalize().into_bytes();
        Ok(format!("{signed}.{}", URL_SAFE_NO_PAD.encode(tag)))
    }

    /// Checks signature (constant time) and expiry, then returns the claims.
    pub fn verify(&self, token: &str, now: u64) -> Result<PreviewClaims, PreviewTokenError> {
        let mut parts = token.split('.');
        let (Some(version), Some(kid), Some(payload), Some(tag), None) =
            (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(PreviewTokenError::Malformed);
        };
        if version != PREVIEW_TOKEN_VERSION {
            return Err(PreviewTokenError::Malformed);
        }
        let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| PreviewTokenError::Malformed)?;
        let key = self.keys.get(kid).ok_or(PreviewTokenError::UnknownKey)?;
        // `verify_slice` compares in constant time.
        mac(key, &format!("{version}.{kid}.{payload}"))?
            .verify_slice(&tag)
            .map_err(|_| PreviewTokenError::BadSignature)?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| PreviewTokenError::Malformed)?;
        let claims: PreviewClaims = serde_json::from_slice(&payload).map_err(|_| PreviewTokenError::Malformed)?;
        if now >= claims.expires_at {
            return Err(PreviewTokenError::Expired);
        }
        Ok(claims)
    }

    /// [`Self::verify`] plus the check that the token was issued for exactly this lease and port.
    pub fn verify_for(
        &self,
        token: &str,
        lease_id: &LeaseId,
        port: u16,
        now: u64,
    ) -> Result<PreviewClaims, PreviewTokenError> {
        let claims = self.verify(token, now)?;
        if &claims.lease_id != lease_id {
            return Err(PreviewTokenError::WrongLease);
        }
        if claims.port != port {
            return Err(PreviewTokenError::WrongPort);
        }
        Ok(claims)
    }
}
