#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TaskId(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid task identifier")]
pub struct TaskIdError;

impl TaskId {
    pub fn new(value: impl Into<String>) -> Result<Self, TaskIdError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(TaskIdError);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
