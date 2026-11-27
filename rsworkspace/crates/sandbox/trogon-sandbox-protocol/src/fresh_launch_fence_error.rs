/// Why the dispatch fence cannot yet guard a fresh command launch.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum FreshLaunchFenceError {
    #[error("fresh command launch requires signature enforcement")]
    VerificationRequired,
    #[error("fresh command launch requires verification keys")]
    VerificationKeysUnavailable,
    #[error("fresh command launch requires epoch enforcement")]
    EpochRequired,
    #[error("fresh command launch requires a persistent dispatch fence")]
    JournalUnavailable,
    #[error("fresh command launch requires a trusted dispatch fence")]
    UntrustedJournal,
    #[error("fresh command launch fence identity is unearned on this platform")]
    UnsupportedPlatform,
    #[error("fresh command launch dispatch fence durability failed")]
    JournalIoFailed,
    #[error("fresh command launch dispatch fence identity is unavailable")]
    JournalIdentityUnavailable,
    #[error("fresh command launch dispatch fence identity or length changed")]
    JournalChanged,
}
