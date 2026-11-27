use rand_core::{OsRng, RngCore};

use crate::EntropyError;

/// `byte_count` random bytes from the operating system, hex encoded.
pub(crate) fn random_hex(byte_count: usize) -> Result<String, EntropyError> {
    let mut raw = vec![0u8; byte_count];
    OsRng.try_fill_bytes(&mut raw)?;
    Ok(hex::encode(raw))
}
