use std::collections::BTreeMap;
use std::path::Path;

use trogon_sandbox_contract::Digest;

use crate::digest_hash::{digest_from_hash, hash_file};
use crate::tree_files::tree_files;
use crate::{ChangedManifest, ChangedManifestEntry, ManifestKey, VolumeError};

/// Lists the files under `src` whose digest differs from `baseline`, or every file when there is
/// no baseline.
pub fn build_changed_manifest(
    src: &Path,
    baseline: Option<&BTreeMap<ManifestKey, Digest>>,
) -> Result<ChangedManifest, VolumeError> {
    let mut entries = Vec::new();
    for (key, path) in tree_files(src)? {
        let digest = digest_from_hash(&hash_file(&path)?)?;
        let unchanged = baseline
            .and_then(|base| base.get(&key))
            .is_some_and(|known| *known == digest);
        if !unchanged {
            let size_bytes = std::fs::metadata(&path)?.len();
            entries.push(ChangedManifestEntry {
                key,
                digest,
                size_bytes,
            });
        }
    }
    Ok(ChangedManifest { entries })
}
