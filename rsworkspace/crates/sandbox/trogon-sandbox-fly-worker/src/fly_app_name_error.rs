/// A Fly app name was empty, longer than 63 bytes, or held a character other than a letter, digit, or `-`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("a Fly app name is 1 to 63 letters, digits, or hyphens")]
pub struct FlyAppNameError;
