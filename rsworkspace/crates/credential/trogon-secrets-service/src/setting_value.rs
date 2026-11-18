use std::fmt;

use trogon_service_config::SecretReference;

/// The effective value of a setting as the configuration dump shows it. A
/// secret is shown only by the reference it is read from, never by what that
/// reference holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingValue {
    /// A value that is not a secret, shown as it is.
    Shown(String),
    /// Where a secret is read from.
    Reference(SecretReference),
    /// A secret given inline, which is never shown.
    Redacted,
    /// No source sets the setting, and it has no default.
    Unset,
}

impl fmt::Display for SettingValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shown(value) => f.write_str(value),
            Self::Reference(reference) => write!(f, "{reference}"),
            Self::Redacted => f.write_str("<redacted>"),
            Self::Unset => f.write_str("<unset>"),
        }
    }
}

#[cfg(test)]
mod tests;
