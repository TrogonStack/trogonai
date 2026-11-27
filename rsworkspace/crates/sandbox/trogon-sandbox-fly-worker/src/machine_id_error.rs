/// A Machine id was empty, longer than 64 bytes, or held a character other than a letter, digit, or `-`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("a Machine id is 1 to 64 letters, digits, or hyphens")]
pub struct MachineIdError;
