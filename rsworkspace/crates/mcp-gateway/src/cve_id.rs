use std::fmt;

/// Identifier for a vulnerability finding: either an upstream `CVE-*`
/// alias (preferred, matching AGT's `_parse_osv_response` alias lookup) or
/// the raw OSV vulnerability id (e.g. `GHSA-...`) when no CVE alias exists.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CveId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CveIdError {
    #[error("CVE/OSV id must not be empty")]
    Empty,
}

impl CveId {
    pub fn new(value: impl Into<String>) -> Result<Self, CveIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(CveIdError::Empty);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CveId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        assert_eq!(CveId::new("").unwrap_err(), CveIdError::Empty);
    }

    #[test]
    fn accepts_cve_alias() {
        assert_eq!(CveId::new("CVE-2024-12345").unwrap().as_str(), "CVE-2024-12345");
    }

    #[test]
    fn accepts_osv_id_when_no_cve_alias() {
        assert_eq!(
            CveId::new("GHSA-xxxx-yyyy-zzzz").unwrap().as_str(),
            "GHSA-xxxx-yyyy-zzzz"
        );
    }
}
