use std::fmt;
use std::str::FromStr;

use crate::VerifyModeError;

/// What a restore does when the signature, or the signed annotations, do not verify.
///
/// `Warn` is the default so existing unsigned snapshots keep restoring while operators roll the
/// public key out; move to `Require` once every snapshot in use is signed.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum VerifyMode {
    /// Do not verify.
    Off,
    /// Verify, log a failure, and restore anyway.
    #[default]
    Warn,
    /// Verify and refuse the restore on any failure, including a missing public key.
    Require,
}

impl FromStr for VerifyMode {
    type Err = VerifyModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "warn" => Ok(Self::Warn),
            "require" => Ok(Self::Require),
            other => Err(VerifyModeError(other.to_string())),
        }
    }
}

impl fmt::Display for VerifyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Warn => "warn",
            Self::Require => "require",
        })
    }
}
