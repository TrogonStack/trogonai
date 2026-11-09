#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ControlRequestId(String);

impl ControlRequestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ControlRequestId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ControlRequestId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for ControlRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
