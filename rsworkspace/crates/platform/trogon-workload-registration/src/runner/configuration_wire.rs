use std::num::NonZeroU64;

#[derive(clap::Args)]
pub(super) struct ConfigurationWire {
    #[arg(long)]
    pub tenant: String,
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub trust_domain: String,
    #[arg(long)]
    pub recovery_fence: NonZeroU64,
    /// Pinned Ed25519 public key as exactly 64 hexadecimal characters.
    #[arg(long)]
    pub approval_public_key: String,
}
