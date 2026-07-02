use std::fmt;

use crate::PackageEcosystem;

/// A registered MCP server package identified by name, version, and
/// ecosystem, the unit the CVE feed gate looks up against OSV.dev.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PackageCoordinate {
    name: String,
    version: String,
    ecosystem: PackageEcosystem,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageCoordinateError {
    #[error("package name must not be empty")]
    EmptyName,
    #[error("package version must not be empty")]
    EmptyVersion,
}

impl PackageCoordinate {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        ecosystem: PackageEcosystem,
    ) -> Result<Self, PackageCoordinateError> {
        let name = name.into();
        let version = version.into();
        if name.is_empty() {
            return Err(PackageCoordinateError::EmptyName);
        }
        if version.is_empty() {
            return Err(PackageCoordinateError::EmptyVersion);
        }
        Ok(Self {
            name,
            version,
            ecosystem,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn ecosystem(&self) -> PackageEcosystem {
        self.ecosystem
    }

    /// Cache key: `ecosystem:name:version`, matching AGT's cache key shape.
    pub fn cache_key(&self) -> String {
        format!("{}:{}:{}", self.ecosystem.as_osv_str(), self.name, self.version)
    }
}

impl fmt::Display for PackageCoordinate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{} ({})", self.name, self.version, self.ecosystem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_name() {
        assert_eq!(
            PackageCoordinate::new("", "1.0.0", PackageEcosystem::Npm).unwrap_err(),
            PackageCoordinateError::EmptyName
        );
    }

    #[test]
    fn rejects_empty_version() {
        assert_eq!(
            PackageCoordinate::new("mcp-server-sqlite", "", PackageEcosystem::Npm).unwrap_err(),
            PackageCoordinateError::EmptyVersion
        );
    }

    #[test]
    fn accepts_valid_coordinate() {
        let coord = PackageCoordinate::new("mcp-server-sqlite", "0.3.1", PackageEcosystem::PyPi).unwrap();
        assert_eq!(coord.name(), "mcp-server-sqlite");
        assert_eq!(coord.version(), "0.3.1");
        assert_eq!(coord.ecosystem(), PackageEcosystem::PyPi);
    }

    #[test]
    fn cache_key_combines_ecosystem_name_version() {
        let coord = PackageCoordinate::new("mcp-server-sqlite", "0.3.1", PackageEcosystem::PyPi).unwrap();
        assert_eq!(coord.cache_key(), "PyPI:mcp-server-sqlite:0.3.1");
    }

    #[test]
    fn display_shows_name_version_and_ecosystem() {
        let coord = PackageCoordinate::new("left-pad", "1.0.0", PackageEcosystem::Npm).unwrap();
        assert_eq!(coord.to_string(), "left-pad@1.0.0 (npm)");
    }
}
