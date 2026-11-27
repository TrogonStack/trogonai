/// Largest PEM file the transport reads, so a corrupt projected volume cannot exhaust memory.
pub(super) const MAX_PEM_BYTES: u64 = 1024 * 1024;
