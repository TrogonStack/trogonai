use crate::ToolName;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BuiltinToolName(String);

impl BuiltinToolName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for BuiltinToolName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for BuiltinToolName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<ToolName> for BuiltinToolName {
    fn from(value: ToolName) -> Self {
        Self::from(value.as_str())
    }
}
