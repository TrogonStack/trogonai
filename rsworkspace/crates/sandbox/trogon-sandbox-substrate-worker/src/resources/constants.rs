/// Longest container image reference accepted.
pub(super) const MAX_IMAGE_REFERENCE_LEN: usize = 512;

/// Largest CPU request, in millicores, that Substrate accepts.
pub(super) const MAX_CPU_MILLICORES: u32 = 999_999;

/// Bytes in one MiB.
pub(super) const BYTES_PER_MIB: i64 = 1024 * 1024;
