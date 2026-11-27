/// The operating system could not supply random bytes.
#[derive(Debug, thiserror::Error)]
#[error("the operating system could not supply random bytes")]
pub struct EntropyError(#[from] rand_core::Error);
