use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use super::error::SignedExportError;
use super::keys::{Ed25519PublicKey, OperatorKeyId};

const SIGNING_DOMAIN: &[u8] = b"a2a.discovery.export.v1\x00";
pub const DEFAULT_SIGNATURE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedExportEnvelope {
    pub key_id: OperatorKeyId,
    pub signed_at_unix_ms: u64,
    pub payload_sha256: [u8; 32],
    pub signature: [u8; 64],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDiscoveryExport {
    pub key_id: OperatorKeyId,
    pub signed_at_unix_ms: u64,
    pub payload_sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureVerificationConfig {
    pub now_unix_ms: u64,
    pub max_age: Duration,
}

impl SignatureVerificationConfig {
    pub fn at_now(now_unix_ms: u64, max_age: Duration) -> Self {
        Self { now_unix_ms, max_age }
    }
}

impl Default for SignatureVerificationConfig {
    fn default() -> Self {
        Self {
            now_unix_ms: 0,
            max_age: DEFAULT_SIGNATURE_MAX_AGE,
        }
    }
}

pub fn payload_sha256(payload_bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(payload_bytes).into()
}

pub fn sign_discovery_export(
    privkey: &SigningKey,
    key_id: OperatorKeyId,
    payload: &[u8],
    now_unix_ms: u64,
) -> SignedExportEnvelope {
    let digest = payload_sha256(payload);
    let message = signing_message(&key_id, now_unix_ms, &digest);
    let signature = privkey.sign(&message);

    SignedExportEnvelope {
        key_id,
        signed_at_unix_ms: now_unix_ms,
        payload_sha256: digest,
        signature: signature.to_bytes(),
    }
}

pub fn verify_signed_export(
    trusted_keys: &std::collections::BTreeMap<OperatorKeyId, Ed25519PublicKey>,
    payload_bytes: &[u8],
    envelope: &SignedExportEnvelope,
    config: &SignatureVerificationConfig,
) -> Result<SignedDiscoveryExport, SignedExportError> {
    let max_age_ms = config.max_age.as_millis().min(u64::MAX as u128) as u64;
    if config.now_unix_ms.saturating_sub(envelope.signed_at_unix_ms) > max_age_ms {
        return Err(SignedExportError::StaleSignature {
            signed_at_unix_ms: envelope.signed_at_unix_ms,
            now_unix_ms: config.now_unix_ms,
            max_age_ms,
        });
    }

    let digest = payload_sha256(payload_bytes);
    if digest != envelope.payload_sha256 {
        return Err(SignedExportError::PayloadDigestMismatch);
    }

    let public_key = trusted_keys
        .get(&envelope.key_id)
        .ok_or_else(|| SignedExportError::UnknownKeyId(envelope.key_id.as_str().to_owned()))?;

    let verifying_key = VerifyingKey::from_bytes(public_key.as_bytes())
        .map_err(|error| SignedExportError::Malformed(format!("invalid ed25519 public key: {error}")))?;

    let signature = ed25519_dalek::Signature::from_bytes(&envelope.signature);
    let message = signing_message(&envelope.key_id, envelope.signed_at_unix_ms, &envelope.payload_sha256);
    verifying_key
        .verify(&message, &signature)
        .map_err(|_| SignedExportError::SignatureMismatch)?;

    Ok(SignedDiscoveryExport {
        key_id: envelope.key_id.clone(),
        signed_at_unix_ms: envelope.signed_at_unix_ms,
        payload_sha256: envelope.payload_sha256,
    })
}

fn signing_message(key_id: &OperatorKeyId, signed_at_unix_ms: u64, payload_sha256: &[u8; 32]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + key_id.as_str().len() + 8 + 32);
    message.extend_from_slice(SIGNING_DOMAIN);
    message.extend_from_slice(key_id.as_str().as_bytes());
    message.extend_from_slice(&signed_at_unix_ms.to_le_bytes());
    message.extend_from_slice(payload_sha256);
    message
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    use super::*;

    fn fixture_keypair() -> (SigningKey, OperatorKeyId, BTreeMap<OperatorKeyId, Ed25519PublicKey>) {
        let signing_key = SigningKey::generate(&mut OsRng);
        let key_id = OperatorKeyId::parse("ops.prod").expect("valid key id");
        let mut trusted = BTreeMap::new();
        trusted.insert(
            key_id.clone(),
            Ed25519PublicKey::from_bytes(signing_key.verifying_key().to_bytes()),
        );
        (signing_key, key_id, trusted)
    }

    #[test]
    fn verify_signed_export_happy_path() {
        let (signing_key, key_id, trusted) = fixture_keypair();
        let payload = br#"{"subject":"a2a.discover.>"}"#;
        let now = 1_700_000_000_000_u64;
        let envelope = sign_discovery_export(&signing_key, key_id, payload, now);
        let config = SignatureVerificationConfig::at_now(now, DEFAULT_SIGNATURE_MAX_AGE);

        let verified = verify_signed_export(&trusted, payload, &envelope, &config).expect("valid export");

        assert_eq!(verified.key_id, envelope.key_id);
        assert_eq!(verified.payload_sha256, envelope.payload_sha256);
    }

    #[test]
    fn verify_signed_export_unknown_key_id() {
        let (signing_key, key_id, _trusted) = fixture_keypair();
        let payload = b"payload";
        let envelope = sign_discovery_export(&signing_key, key_id, payload, 1);
        let empty = BTreeMap::new();

        let err = verify_signed_export(&empty, payload, &envelope, &SignatureVerificationConfig::at_now(1, DEFAULT_SIGNATURE_MAX_AGE))
            .expect_err("missing trusted key");

        assert!(matches!(err, SignedExportError::UnknownKeyId(_)));
    }

    #[test]
    fn verify_signed_export_signature_mismatch() {
        let (signing_key, key_id, trusted) = fixture_keypair();
        let payload = b"payload";
        let mut envelope = sign_discovery_export(&signing_key, key_id, payload, 1);
        envelope.signature[0] ^= 0xFF;

        let err = verify_signed_export(&trusted, payload, &envelope, &SignatureVerificationConfig::at_now(1, DEFAULT_SIGNATURE_MAX_AGE))
            .expect_err("tampered signature");

        assert!(matches!(err, SignedExportError::SignatureMismatch));
    }

    #[test]
    fn verify_signed_export_payload_digest_mismatch() {
        let (signing_key, key_id, trusted) = fixture_keypair();
        let payload = b"payload";
        let envelope = sign_discovery_export(&signing_key, key_id, payload, 1);

        let err = verify_signed_export(
            &trusted,
            b"tampered",
            &envelope,
            &SignatureVerificationConfig::at_now(1, DEFAULT_SIGNATURE_MAX_AGE),
        )
        .expect_err("payload mismatch");

        assert!(matches!(err, SignedExportError::PayloadDigestMismatch));
    }

    #[test]
    fn verify_signed_export_stale_signature() {
        let (signing_key, key_id, trusted) = fixture_keypair();
        let payload = b"payload";
        let signed_at = 1_000_u64;
        let envelope = sign_discovery_export(&signing_key, key_id, payload, signed_at);
        let max_age = Duration::from_secs(60);
        let now = signed_at + max_age.as_millis() as u64 + 1;

        let err = verify_signed_export(
            &trusted,
            payload,
            &envelope,
            &SignatureVerificationConfig::at_now(now, max_age),
        )
        .expect_err("stale signature");

        assert!(matches!(err, SignedExportError::StaleSignature { .. }));
    }

    #[test]
    fn operator_key_id_rejects_invalid_values() {
        assert!(OperatorKeyId::parse("").is_err());
        assert!(OperatorKeyId::parse("bad space").is_err());
        assert!(OperatorKeyId::parse("x".repeat(65)).is_err());
        assert!(OperatorKeyId::parse("valid-key_1.prod").is_ok());
    }
}
