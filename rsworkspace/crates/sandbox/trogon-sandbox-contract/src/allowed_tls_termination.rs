use serde::{Deserialize, Serialize};

/// Whether the TLS of every allowed CONNECT to a named host is terminated so each request inside can be filtered, whatever the host's domain rule says.
///
/// It serializes as a JSON boolean, `true` for `Enabled`, and an omitted value reads as `Disabled`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum AllowedTlsTermination {
    #[default]
    Disabled,
    Enabled,
}

impl AllowedTlsTermination {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl From<bool> for AllowedTlsTermination {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<AllowedTlsTermination> for bool {
    fn from(flag: AllowedTlsTermination) -> Self {
        flag.is_enabled()
    }
}
