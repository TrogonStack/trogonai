use std::fmt;

use super::{ConfigSetting, SettingSource, SettingValue};

/// One setting of the configuration dump: its effective value and the source
/// that value comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSetting {
    pub setting: ConfigSetting,
    pub value: SettingValue,
    pub source: SettingSource,
}

impl fmt::Display for ResolvedSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} = {} (from {})", self.setting, self.value, self.source)
    }
}

#[cfg(test)]
mod tests;
