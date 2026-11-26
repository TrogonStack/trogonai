use std::fmt;

/// A canonical DNS name: lowercase, ASCII (IDNA punycode), no trailing dot.
///
/// It has no public constructor. A [`Host`](crate::Host) or [`DomainPattern`](crate::DomainPattern) carries one
/// after it has parsed its input.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct DomainName(String);

impl DomainName {
    /// Longest DNS name, in bytes.
    pub const MAX_LEN: usize = 253;
    /// Longest label of a DNS name, in bytes.
    pub const MAX_LABEL_LEN: usize = 63;

    /// Wraps `name` when it is a well-formed DNS name. The caller has already canonicalized it.
    pub(crate) fn new(name: &str) -> Option<Self> {
        let valid = !name.is_empty()
            && name.len() <= Self::MAX_LEN
            && name.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= Self::MAX_LABEL_LEN
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            });
        valid.then(|| Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reports whether this name is a label sequence below `suffix`, never `suffix` itself.
    pub(crate) fn is_strict_subdomain_of(&self, suffix: &Self) -> bool {
        self.0
            .strip_suffix(suffix.as_str())
            .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.'))
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
