use std::fmt;

/// Where the effective value of a setting comes from. Each source is read over
/// the one before it (ADR#0007).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingSource {
    /// The built-in default, which leaves most settings unset.
    Default,
    /// The configuration file named by `--config`.
    File,
    /// The environment variable of the setting.
    Environment,
    /// The command line flag of the setting.
    Flag,
}

impl fmt::Display for SettingSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Default => "default",
            Self::File => "file",
            Self::Environment => "environment",
            Self::Flag => "flag",
        })
    }
}

#[cfg(test)]
mod tests;
