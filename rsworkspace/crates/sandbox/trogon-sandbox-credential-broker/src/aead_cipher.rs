use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rand_core::{OsRng, RngCore};

use crate::KmsError;
use crate::constants::{KEY_LEN, NONCE_LEN};

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

pub(crate) fn aead_encrypt(
    key: &[u8; KEY_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<(Vec<u8>, [u8; NONCE_LEN]), KmsError> {
    let nonce = random_bytes::<NONCE_LEN>();
    let ciphertext = Aes256Gcm::new(&(*key).into())
        .encrypt(&Nonce::from(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| KmsError::Encrypt)?;
    Ok((ciphertext, nonce))
}

pub(crate) fn aead_decrypt(
    key: &[u8; KEY_LEN],
    aad: &[u8],
    ciphertext: &[u8],
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>, KmsError> {
    Aes256Gcm::new(&(*key).into())
        .decrypt(&Nonce::from(*nonce), Payload { msg: ciphertext, aad })
        .map_err(|_| KmsError::Decrypt)
}
