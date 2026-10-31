use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnvVarName(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnvVarNameError {
    #[error("the variable name is empty")]
    Empty,
    #[error("the variable name contains '=' or NUL, which no variable name can hold")]
    ReservedCharacter,
}

impl EnvVarName {
    pub fn new(name: impl Into<String>) -> Result<Self, EnvVarNameError> {
        let name = name.into();
        if name.is_empty() {
            return Err(EnvVarNameError::Empty);
        }
        if name.contains(['=', '\0']) {
            return Err(EnvVarNameError::ReservedCharacter);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EnvVarName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
