use std::fmt;

/// Package ecosystem understood by the OSV.dev API, restricted to the set
/// AGT's `McpCveFeed` supports for MCP server packages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackageEcosystem {
    Npm,
    PyPi,
    CratesIo,
    Go,
}

impl PackageEcosystem {
    /// OSV.dev's expected ecosystem identifier string.
    pub fn as_osv_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::PyPi => "PyPI",
            Self::CratesIo => "crates.io",
            Self::Go => "Go",
        }
    }
}

impl fmt::Display for PackageEcosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_osv_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_osv_ecosystem_identifiers() {
        assert_eq!(PackageEcosystem::Npm.as_osv_str(), "npm");
        assert_eq!(PackageEcosystem::PyPi.as_osv_str(), "PyPI");
        assert_eq!(PackageEcosystem::CratesIo.as_osv_str(), "crates.io");
        assert_eq!(PackageEcosystem::Go.as_osv_str(), "Go");
    }

    #[test]
    fn display_matches_osv_str() {
        assert_eq!(PackageEcosystem::Npm.to_string(), "npm");
    }
}
