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
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn new_wraps_existing_uuid() {
        let uuid = uuid::Uuid::nil();
        assert_eq!(AcpConnectionId::new(uuid).to_string(), uuid.to_string());
    }

    #[test]
    fn parse_round_trips_uuid() {
        let id = AcpConnectionId::default();
        let parsed = AcpConnectionId::parse(&id.to_string()).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_rejects_invalid_uuid() {
        assert!(AcpConnectionId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn default_generates_non_empty_id() {
        assert!(!AcpConnectionId::default().to_string().is_empty());
    }

    #[test]
    fn parse_error_displays_context() {
        let error = AcpConnectionId::parse("not-a-uuid").unwrap_err();
        assert!(error.to_string().contains("invalid ACP connection id"));
    }

    #[test]
    fn parse_error_exposes_uuid_source() {
        let error = AcpConnectionId::parse("not-a-uuid").unwrap_err();
        assert!(error.source().is_some());
    }
}
