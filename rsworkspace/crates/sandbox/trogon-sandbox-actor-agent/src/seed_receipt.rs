use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::Digest;

use crate::{
    EntropyReseedError, EntropyReseeder, GuestEntropyReseedMethod, GuestEntropyReseedReceipt, GuestEntropySeed,
};

/// Applies `seed` through `reseeder` and returns the receipt that names it by digest.
pub(crate) fn apply_seed(
    reseeder: &dyn EntropyReseeder,
    seed: &GuestEntropySeed,
) -> Result<GuestEntropyReseedReceipt, EntropyReseedError> {
    reseeder.reseed(seed)?;
    Ok(GuestEntropyReseedReceipt {
        seed_digest: Digest::new(sha256_label(seed.as_bytes()))?,
        method: GuestEntropyReseedMethod::LinuxKernelV1,
    })
}

fn sha256_label(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}
