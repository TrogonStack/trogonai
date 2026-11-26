#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TaskId(String);

impl TaskId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for TaskId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for TaskId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
