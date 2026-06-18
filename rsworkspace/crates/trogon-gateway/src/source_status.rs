use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceStatus {
    #[default]
    Enabled,
    Disabled,
}

impl SourceStatus {
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl FromStr for SourceStatus {
    type Err = SourceStatusError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            _ => Err(SourceStatusError::new(value)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unsupported status value '{value}' ; expected 'enabled' or 'disabled'")]
pub struct SourceStatusError {
    value: String,
}

impl SourceStatusError {
    pub fn new(value: impl Into<String>) -> Self {
        Self { value: value.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled() {
        assert_eq!(SourceStatus::default(), SourceStatus::Enabled);
    }

    #[test]
    fn parses_enabled_and_disabled_case_insensitively() {
        assert_eq!("enabled".parse::<SourceStatus>().unwrap(), SourceStatus::Enabled);
        assert_eq!("DISABLED".parse::<SourceStatus>().unwrap(), SourceStatus::Disabled);
    }

    #[test]
    fn rejects_unknown_values() {
        let err = "maybe".parse::<SourceStatus>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "unsupported status value 'maybe' ; expected 'enabled' or 'disabled'"
        );
    }
}
