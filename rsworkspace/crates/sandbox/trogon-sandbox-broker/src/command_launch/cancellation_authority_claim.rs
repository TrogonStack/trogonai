use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancellationAuthorityClaim {
    New,
    Existing(CommandCancellationAuthority),
}
