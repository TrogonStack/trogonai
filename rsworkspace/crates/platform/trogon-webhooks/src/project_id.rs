use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectId(String);

#[derive(Debug, thiserror::Error)]
#[error("project id must contain 1 to 64 ASCII letters, digits, underscores or hyphens")]
pub struct ProjectIdError;

impl ProjectId {
    pub fn new(value: &str) -> Result<Self, ProjectIdError> {
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(ProjectIdError);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
