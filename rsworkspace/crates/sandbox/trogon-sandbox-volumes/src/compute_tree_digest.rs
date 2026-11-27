use std::path::Path;

use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::Digest;

use crate::VolumeError;
use crate::digest_hash::{digest_from_hash, hash_file};
use crate::tree_files::tree_files;

/// A digest over every file's key and content under `root`, independent of directory order.
pub fn compute_tree_digest(root: &Path) -> Result<Digest, VolumeError> {
    let mut hasher = Sha256::new();
    for (key, path) in tree_files(root)? {
        hasher.update(key.as_str().as_bytes());
        hasher.update(hash_file(&path)?);
    }
    digest_from_hash(&hasher.finalize())
}
