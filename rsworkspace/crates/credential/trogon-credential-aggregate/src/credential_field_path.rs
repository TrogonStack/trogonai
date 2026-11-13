use std::borrow::Cow;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialFieldPath(Cow<'static, str>);

impl CredentialFieldPath {
    pub fn child(&self, name: &'static str) -> Self {
        Self(Cow::Owned(format!("{}.{name}", self.0)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for CredentialFieldPath {
    fn from(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }
}

impl From<&CredentialFieldPath> for CredentialFieldPath {
    fn from(value: &CredentialFieldPath) -> Self {
        value.clone()
    }
}

impl PartialEq<&str> for CredentialFieldPath {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for CredentialFieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
