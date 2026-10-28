use std::collections::BTreeSet;
use std::fmt;

const MAX_INJECTION_NAME_LEN: usize = 128;

/// Where a resolved credential may be placed in an outbound provider request.
///
/// Restricted to the three placements every provider integration actually uses.
/// The point of enumerating them is negative: a credential configured for a
/// header must not become a query parameter, where it would land in provider
/// access logs.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InjectionLocation {
    Header(String),
    QueryParameter(String),
    BodyField(String),
}

impl InjectionLocation {
    pub fn header(name: impl AsRef<str>) -> Result<Self, InjectionLocationError> {
        let name = name.as_ref();
        validate_name(name)?;
        for ch in name.chars() {
            if !is_header_token_char(ch) {
                return Err(InjectionLocationError::InvalidHeaderCharacter(ch));
            }
        }
        Ok(Self::Header(name.to_ascii_lowercase()))
    }

    pub fn query_parameter(name: impl AsRef<str>) -> Result<Self, InjectionLocationError> {
        let name = name.as_ref();
        validate_name(name)?;
        for ch in name.chars() {
            if !is_query_name_char(ch) {
                return Err(InjectionLocationError::InvalidQueryCharacter(ch));
            }
        }
        Ok(Self::QueryParameter(name.to_string()))
    }

    pub fn body_field(path: impl AsRef<str>) -> Result<Self, InjectionLocationError> {
        let path = path.as_ref();
        validate_name(path)?;
        for segment in path.split('.') {
            if segment.is_empty() {
                return Err(InjectionLocationError::EmptyBodyFieldSegment);
            }
            for ch in segment.chars() {
                if !is_body_field_char(ch) {
                    return Err(InjectionLocationError::InvalidBodyFieldCharacter(ch));
                }
            }
        }
        Ok(Self::BodyField(path.to_string()))
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Header(name) | Self::QueryParameter(name) | Self::BodyField(name) => name,
        }
    }
}

impl fmt::Display for InjectionLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header(name) => write!(f, "header:{name}"),
            Self::QueryParameter(name) => write!(f, "query:{name}"),
            Self::BodyField(path) => write!(f, "body:{path}"),
        }
    }
}

/// The placement restriction on a credential.
///
/// Unlike `AllowedHosts` and `AllowedRuntimeServices`, the default here is an
/// empty `Only` set rather than `Unrestricted`: placement is a property the
/// credential's owner configures, and a credential with no configured placement
/// should not be injectable anywhere by accident.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InjectionLocations(BTreeSet<InjectionLocation>);

impl InjectionLocations {
    pub fn new<I>(locations: I) -> Self
    where
        I: IntoIterator<Item = InjectionLocation>,
    {
        Self(locations.into_iter().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn permits(&self, location: &InjectionLocation) -> bool {
        self.0.contains(location)
    }

    pub fn iter(&self) -> impl Iterator<Item = &InjectionLocation> {
        self.0.iter()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InjectionLocationError {
    #[error("injection location name must not be empty")]
    Empty,
    #[error("injection location name exceeds maximum length: {0}")]
    TooLong(usize),
    #[error("header name contains invalid character '{0}'")]
    InvalidHeaderCharacter(char),
    #[error("query parameter name contains invalid character '{0}'")]
    InvalidQueryCharacter(char),
    #[error("body field path contains invalid character '{0}'")]
    InvalidBodyFieldCharacter(char),
    #[error("body field path must not contain an empty segment")]
    EmptyBodyFieldSegment,
}

fn validate_name(value: &str) -> Result<(), InjectionLocationError> {
    if value.is_empty() {
        return Err(InjectionLocationError::Empty);
    }
    let char_count = value.chars().count();
    if char_count > MAX_INJECTION_NAME_LEN {
        return Err(InjectionLocationError::TooLong(char_count));
    }
    Ok(())
}

/// RFC 9110 field-name token characters.
fn is_header_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '!' | '#' | '$' | '%' | '&' | '\'' | '*' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
        )
}

fn is_query_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~')
}

fn is_body_field_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_names_normalize_to_lowercase() {
        let location = InjectionLocation::header("Authorization").unwrap();

        assert_eq!(location.name(), "authorization");
        assert_eq!(location.to_string(), "header:authorization");
    }

    #[test]
    fn header_and_query_of_the_same_name_are_different_locations() {
        let header = InjectionLocation::header("token").unwrap();
        let query = InjectionLocation::query_parameter("token").unwrap();

        assert_ne!(header, query);

        let locations = InjectionLocations::new([header.clone()]);
        assert!(locations.permits(&header));
        assert!(!locations.permits(&query));
    }

    #[test]
    fn rejects_names_that_would_need_escaping() {
        assert_eq!(InjectionLocation::header(""), Err(InjectionLocationError::Empty));
        assert_eq!(
            InjectionLocation::header("X Api Key"),
            Err(InjectionLocationError::InvalidHeaderCharacter(' '))
        );
        assert_eq!(
            InjectionLocation::header("X-Api-Key\r\nInjected"),
            Err(InjectionLocationError::InvalidHeaderCharacter('\r'))
        );
        assert_eq!(
            InjectionLocation::query_parameter("api key"),
            Err(InjectionLocationError::InvalidQueryCharacter(' '))
        );
        assert_eq!(
            InjectionLocation::body_field("auth..token"),
            Err(InjectionLocationError::EmptyBodyFieldSegment)
        );
    }

    #[test]
    fn body_field_paths_are_dotted_segments() {
        let location = InjectionLocation::body_field("auth.api_key").unwrap();

        assert_eq!(location.to_string(), "body:auth.api_key");
    }

    #[test]
    fn default_permits_no_placement() {
        let locations = InjectionLocations::default();
        let header = InjectionLocation::header("authorization").unwrap();

        assert!(locations.is_empty());
        assert!(!locations.permits(&header));
    }
}
