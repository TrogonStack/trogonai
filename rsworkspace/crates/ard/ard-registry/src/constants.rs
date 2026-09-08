//! Crate-wide constants.

pub const DEFAULT_PAGE_SIZE: u32 = 50;
pub const MAX_PAGE_SIZE: u32 = 100;

/// Version tag embedded in encoded pagination tokens.
pub(crate) const PAGE_TOKEN_VERSION: u8 = 1;
