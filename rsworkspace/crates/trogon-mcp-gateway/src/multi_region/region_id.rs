use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionIdError {
    Empty,
}

impl fmt::Display for RegionIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("region id must not be empty"),
        }
    }
}

impl std::error::Error for RegionIdError {}

impl RegionId {
    pub fn new(id: impl Into<String>) -> Result<Self, RegionIdError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(RegionIdError::Empty);
        }
        Ok(Self(id))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for RegionId {
    type Error = RegionIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
