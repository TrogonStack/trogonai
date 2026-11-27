/// A Fly volume id did not start with `vol_` or held a character other than a letter, digit, or `_`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("a Fly volume id is `vol_` followed by letters, digits, or underscores")]
pub struct FlyVolumeIdError;
