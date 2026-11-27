use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// The caller owns the key and must dispatch, then `finish` or `forget`.
    Claimed,
    /// Another call already claimed the key.
    Existing(CommandLaunch),
}
