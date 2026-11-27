use sigstore::crypto::SigningScheme;

use crate::CosignError;
use crate::failure::sign_failed;

/// Generate an ephemeral ECDSA key pair for tests; returns (private_pem, public_pem).
pub fn generate_test_key_pair() -> Result<(Vec<u8>, Vec<u8>), CosignError> {
    let signer = SigningScheme::ECDSA_P256_SHA256_ASN1
        .create_signer()
        .map_err(sign_failed("create test signer"))?;
    let key_pair = signer.to_sigstore_keypair().map_err(sign_failed("export key pair"))?;
    let private = key_pair
        .private_key_to_pem()
        .map_err(sign_failed("export private key"))?;
    let public = key_pair.public_key_to_pem().map_err(sign_failed("export public key"))?;
    Ok((private.as_bytes().to_vec(), public.into_bytes()))
}
