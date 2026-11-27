use std::str::FromStr;

use crate::EpochModeError;

/// What a worker does with a request carrying a lease epoch lower than the one it holds for that lease.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub enum EpochMode {
    /// Track and persist the epoch; never log or refuse.
    #[default]
    Off,
    /// Log a lower epoch and accept it.
    Warn,
    /// Refuse a lower epoch. Only effective while [`EnforcementMode::Require`](crate::EnforcementMode::Require) is
    /// on; without verified signatures it behaves as `warn`.
    Require,
}

impl EpochMode {
    /// Parses `off`, `warn`, or `require`, ignoring case and surrounding space. An empty value means `off`.
    pub fn parse(value: &str) -> Result<Self, EpochModeError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "" => Ok(Self::Off),
            "warn" => Ok(Self::Warn),
            "require" => Ok(Self::Require),
            other => Err(EpochModeError {
                found: other.to_owned(),
            }),
        }
    }

    /// The mode for an unset or set `SANDBOX_FENCE_EPOCH`; unset means `warn`.
    pub(crate) fn from_env_value(value: Option<&str>) -> Result<Self, EpochModeError> {
        value.map_or(Ok(Self::Warn), Self::parse)
    }
}

impl FromStr for EpochMode {
    type Err = EpochModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
