use crate::SensitiveString;

/// A configuration value that is sensitive unless explicitly marked plain.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ConfigValue {
    Plain(String),
    Sensitive(SensitiveString),
}

impl ConfigValue {
    pub fn expose(&self) -> &str {
        match self {
            Self::Plain(value) => value,
            Self::Sensitive(value) => value.expose(),
        }
    }

    pub fn is_sensitive(&self) -> bool {
        matches!(self, Self::Sensitive(_))
    }
}

impl From<String> for ConfigValue {
    fn from(value: String) -> Self {
        Self::Sensitive(SensitiveString::from(value))
    }
}

impl From<&str> for ConfigValue {
    fn from(value: &str) -> Self {
        Self::Sensitive(SensitiveString::from(value))
    }
}
