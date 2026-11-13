#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DialogKind(String);

impl DialogKind {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for DialogKind {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DialogKind {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
