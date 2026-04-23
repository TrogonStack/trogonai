#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AcpConnectionId(uuid::Uuid);

impl AcpConnectionId {
    pub fn new(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }

    pub fn parse(s: &str) -> Result<Self, AcpConnectionIdError> {
        uuid::Uuid::parse_str(s)
            .map(Self::new)
            .map_err(AcpConnectionIdError::InvalidUuid)
    }
}

impl Default for AcpConnectionId {
    fn default() -> Self {
        Self::new(uuid::Uuid::now_v7())
    }
}

impl std::fmt::Display for AcpConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug)]
pub enum AcpConnectionIdError {
    InvalidUuid(uuid::Error),
}

impl std::fmt::Display for AcpConnectionIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUuid(error) => write!(f, "invalid ACP connection id: {error}"),
        }
    }
}

impl std::error::Error for AcpConnectionIdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUuid(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests;
