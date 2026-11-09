use crate::ToolName;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BuiltinToolName(String);

impl BuiltinToolName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<ToolName> for BuiltinToolName {
    fn from(value: ToolName) -> Self {
        Self::new(value.as_str())
    }
}
