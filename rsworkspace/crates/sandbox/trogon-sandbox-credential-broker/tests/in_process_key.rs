use std::error::Error;

use trogon_sandbox_credential_broker::{EnvelopeKms, InProcessKey, KmsError, KmsProvider, NONCE_LEN};

const KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

#[tokio::test]
async fn a_key_parsed_from_base64_round_trips_with_matching_aad() -> Result<(), Box<dyn Error>> {
    let key: InProcessKey = format!(" {KEY}\n").parse()?;
    let (ciphertext, nonce) = key.encrypt(b"aad", b"secret material").await?;
    assert_eq!(nonce.len(), NONCE_LEN);
    assert_eq!(key.decrypt(b"aad", &ciphertext, &nonce).await?, b"secret material");
    Ok(())
}

#[tokio::test]
async fn the_wrong_aad_nonce_or_key_fails() -> Result<(), Box<dyn Error>> {
    let key = InProcessKey::from_bytes([7u8; 32]);
    let (ciphertext, nonce) = key.encrypt(b"aad", b"secret material").await?;
    assert!(matches!(
        key.decrypt(b"other", &ciphertext, &nonce).await,
        Err(KmsError::Decrypt)
    ));
    assert!(key.decrypt(b"aad", &ciphertext, &[0u8; NONCE_LEN]).await.is_err());
    let other = InProcessKey::from_bytes([8u8; 32]);
    assert!(other.decrypt(b"aad", &ciphertext, &nonce).await.is_err());
    Ok(())
}

#[tokio::test]
async fn the_envelope_kms_wraps_the_same_root_key() -> Result<(), Box<dyn Error>> {
    let kms = EnvelopeKms::new(InProcessKey::from_bytes([7u8; 32]));
    let (ciphertext, nonce) = kms.encrypt(b"aad", b"x").await?;
    assert_eq!(kms.decrypt(b"aad", &ciphertext, &nonce).await?, b"x");
    Ok(())
}

#[test]
fn a_malformed_key_is_rejected_with_a_typed_error() {
    let not_base64 = "***".parse::<InProcessKey>();
    assert!(matches!(not_base64, Err(KmsError::InvalidEncoding(_))));
    let short = "AAAA".parse::<InProcessKey>();
    assert!(matches!(
        short,
        Err(KmsError::InvalidKeyLength {
            expected: 32,
            actual: 3
        })
    ));
}

#[test]
fn error_messages_and_debug_never_leak_the_key() -> Result<(), Box<dyn Error>> {
    let key: InProcessKey = KEY.parse()?;
    assert_eq!(format!("{key:?}"), "InProcessKey([REDACTED])");
    let messages = [
        "***".parse::<InProcessKey>().err().ok_or("parsed")?.to_string(),
        "AAAA".parse::<InProcessKey>().err().ok_or("parsed")?.to_string(),
        KmsError::Encrypt.to_string(),
        KmsError::Decrypt.to_string(),
    ];
    assert_eq!(messages[0], "the key is not valid base64");
    assert_eq!(messages[1], "the key must decode to 32 bytes, got 3");
    assert_eq!(messages[2], "encryption failed");
    assert_eq!(messages[3], "decryption failed");
    Ok(())
}
