use serde::Serialize;

use crate::PackageCoordinate;

/// Request body for `POST https://api.osv.dev/v1/query`, mirroring AGT's
/// `_query_osv` payload shape (`{"version": ..., "package": {"name":
/// ..., "ecosystem": ...}}`).
#[derive(Debug, Serialize)]
pub struct OsvQueryWire {
    version: String,
    package: OsvPackageWire,
}

#[derive(Debug, Serialize)]
struct OsvPackageWire {
    name: String,
    ecosystem: String,
}

impl OsvQueryWire {
    pub fn from_coordinate(coordinate: &PackageCoordinate) -> Self {
        Self {
            version: coordinate.version().to_string(),
            package: OsvPackageWire {
                name: coordinate.name().to_string(),
                ecosystem: coordinate.ecosystem().as_osv_str().to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PackageEcosystem;

    #[test]
    fn serializes_to_osv_query_shape() {
        let coordinate = PackageCoordinate::new("mcp-server-sqlite", "0.3.1", PackageEcosystem::PyPi).expect("valid");
        let wire = OsvQueryWire::from_coordinate(&coordinate);
        let json = serde_json::to_value(&wire).expect("serializable");
        assert_eq!(
            json,
            serde_json::json!({
                "version": "0.3.1",
                "package": {
                    "name": "mcp-server-sqlite",
                    "ecosystem": "PyPI",
                }
            })
        );
    }
}
