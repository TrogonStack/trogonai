//! AES-256-GCM encryption with Argon2id key derivation.
//!
//! Wire format: `[12-byte nonce][ciphertext+tag]`
//! The nonce is randomly generated per operation (OsRng).

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, AeadCore},
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::rngs::OsRng;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CryptoError {
    KeyDerivation(String),
    Encrypt,
    Decrypt,
    InvalidCiphertext,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyDerivation(msg) => write!(f, "key derivation failed: {msg}"),
            Self::Encrypt            => write!(f, "AES-GCM encryption failed"),
            Self::Decrypt            => write!(f, "AES-GCM decryption failed (tag mismatch or corrupt data)"),
            Self::InvalidCiphertext  => write!(f, "ciphertext too short (minimum 12 bytes for nonce)"),
        }
    }
}

impl std::error::Error for CryptoError {}

// ── CryptoCtx ─────────────────────────────────────────────────────────────────

/// Holds a derived 256-bit AES key. Create once at startup, share via `Arc`.
pub struct CryptoCtx {
    key: [u8; 32],
}

impl CryptoCtx {
    /// Derive the master key from `password` and `salt` using Argon2id.
    ///
    /// Parameters: m=64 MiB, t=3 iterations, p=4 lanes.
    /// The `salt` must be at least 8 bytes; 16 bytes (128 bits) is recommended.
    pub fn derive(password: &[u8], salt: &[u8]) -> Result<Self, CryptoError> {
        let params = Params::new(65536, 3, 4, Some(32))
            .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut key = [0u8; 32];
        argon2
            .hash_password_into(password, salt, &mut key)
            .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;

        Ok(Self { key })
    }

    /// Encrypt `plaintext` with AES-256-GCM.
    ///
    /// Returns `[nonce (12 bytes) || ciphertext || tag (16 bytes)]`.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let cipher = Aes256Gcm::new((&self.key).into());
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| CryptoError::Encrypt)?;

        let mut out = Vec::with_capacity(12 + ciphertext.len());
        out.extend_from_slice(&nonce);
        out.extend(ciphertext);
        Ok(out)
    }

    /// Decrypt a blob produced by [`encrypt`](Self::encrypt).
    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if data.len() < 12 {
            return Err(CryptoError::InvalidCiphertext);
        }
        let (nonce_bytes, ciphertext) = data.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let cipher = Aes256Gcm::new((&self.key).into());

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| CryptoError::Decrypt)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CryptoCtx {
        CryptoCtx::derive(b"test-password", b"0123456789abcdef").unwrap()
    }

    #[test]
    fn roundtrip() {
        let ctx = ctx();
        let plaintext = b"sk-ant-api-key-value-here";
        let ciphertext = ctx.encrypt(plaintext).unwrap();
        let recovered = ctx.decrypt(&ciphertext).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn nonces_are_unique() {
        let ctx = ctx();
        let a = ctx.encrypt(b"same").unwrap();
        let b = ctx.encrypt(b"same").unwrap();
        assert_ne!(&a[..12], &b[..12], "nonces must differ across calls");
        assert_ne!(a, b, "ciphertexts must differ (different nonces)");
    }

    #[test]
    fn tampered_ciphertext_fails_decrypt() {
        let ctx = ctx();
        let mut blob = ctx.encrypt(b"secret").unwrap();
        *blob.last_mut().unwrap() ^= 0xff;
        assert!(matches!(ctx.decrypt(&blob), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn too_short_ciphertext_fails() {
        let ctx = ctx();
        assert!(matches!(ctx.decrypt(&[0u8; 5]), Err(CryptoError::InvalidCiphertext)));
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let ctx_a = ctx();
        let ctx_b = CryptoCtx::derive(b"different-password", b"0123456789abcdef").unwrap();
        let blob = ctx_a.encrypt(b"secret").unwrap();
        assert!(matches!(ctx_b.decrypt(&blob), Err(CryptoError::Decrypt)));
    }

    #[test]
    fn derive_deterministic_for_same_input() {
        let a = CryptoCtx::derive(b"pw", b"salt1234").unwrap();
        let b = CryptoCtx::derive(b"pw", b"salt1234").unwrap();
        assert_eq!(a.key, b.key);
    }

    #[test]
    fn derive_differs_for_different_salt() {
        let a = CryptoCtx::derive(b"pw", b"salt1234").unwrap();
        let b = CryptoCtx::derive(b"pw", b"salt5678").unwrap();
        assert_ne!(a.key, b.key);
    }
}
