use super::constants::{TAR_BLOCK_BYTES, TAR_NAME_BYTES};

/// Bytes a tar stream spends on one entry of `size` content bytes named `path`.
pub(super) fn tar_entry_bytes(path: &str, size: u64) -> u64 {
    let padded = |bytes: u64| bytes.saturating_add(TAR_BLOCK_BYTES - 1) / TAR_BLOCK_BYTES * TAR_BLOCK_BYTES;
    let name = if path.len() > TAR_NAME_BYTES {
        TAR_BLOCK_BYTES + padded(path.len() as u64 + 1)
    } else {
        0
    };
    TAR_BLOCK_BYTES + name + padded(size)
}
