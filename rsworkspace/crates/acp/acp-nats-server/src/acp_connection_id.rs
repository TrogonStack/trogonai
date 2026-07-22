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

#[derive(Debug, thiserror::Error)]
pub enum AcpConnectionIdError {
    #[error("invalid ACP connection id: {0}")]
    InvalidUuid(#[source] uuid::Error),
}

#[cfg(test)]
mod tests;
