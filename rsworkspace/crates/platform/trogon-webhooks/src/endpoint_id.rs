use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointId(String);

#[derive(Debug, thiserror::Error)]
pub enum EndpointIdError {
    #[error("endpoint id must be a canonical non-nil UUID")]
    NonCanonical,
    #[error("endpoint id must be a UUID")]
    Invalid(#[source] uuid::Error),
}

impl EndpointId {
    pub fn new(value: &str) -> Result<Self, EndpointIdError> {
        let parsed = uuid::Uuid::parse_str(value).map_err(EndpointIdError::Invalid)?;
        if parsed.is_nil() || parsed.to_string() != value {
            return Err(EndpointIdError::NonCanonical);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
