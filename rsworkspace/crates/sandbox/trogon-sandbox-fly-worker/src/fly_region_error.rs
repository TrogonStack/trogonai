/// A Fly region code was empty or held a character other than a letter, digit, `_`, or `-`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("a Fly region is letters, digits, underscores, or hyphens")]
pub struct FlyRegionError;
