use super::Handle;

pub(super) const HANDLE_FLAG_INHERIT: u32 = 1;

pub(super) const DUPLICATE_SAME_ACCESS: u32 = 2;

pub(super) const ERROR_ACCESS_DENIED: i32 = 5;

pub(super) const INVALID_HANDLE: Handle = -1;

pub(super) const NO_ASSOCIATED_STREAM: Handle = -2;
