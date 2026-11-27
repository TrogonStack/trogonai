use crate::GuestEntropySeed;

/// Mixes a seed into the guest kernel's random pool.
pub trait EntropyReseeder: Send + Sync {
    fn reseed(&self, seed: &GuestEntropySeed) -> std::io::Result<()>;
}
