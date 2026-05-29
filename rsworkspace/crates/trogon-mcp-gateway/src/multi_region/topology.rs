use std::collections::BTreeMap;

use super::region_id::{RegionId, RegionIdError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionEndpoint {
    pub nats_url: String,
    pub creds_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionTopology {
    home_region: RegionId,
    failover_order: Vec<RegionId>,
    endpoints: BTreeMap<RegionId, RegionEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyBuildError {
    HomeNotInEndpoints,
    FailoverIncludesHome,
    RegionId(RegionIdError),
}

impl std::fmt::Display for TopologyBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeNotInEndpoints => f.write_str("home region missing from endpoints"),
            Self::FailoverIncludesHome => f.write_str("failover list must not include home region"),
            Self::RegionId(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TopologyBuildError {}

impl RegionTopology {
    pub fn new(
        home_region: RegionId,
        failover_order: Vec<RegionId>,
        endpoints: BTreeMap<RegionId, RegionEndpoint>,
    ) -> Result<Self, TopologyBuildError> {
        if !endpoints.contains_key(&home_region) {
            return Err(TopologyBuildError::HomeNotInEndpoints);
        }
        if failover_order.iter().any(|r| r == &home_region) {
            return Err(TopologyBuildError::FailoverIncludesHome);
        }
        Ok(Self {
            home_region,
            failover_order,
            endpoints,
        })
    }

    #[must_use]
    pub fn home_region(&self) -> &RegionId {
        &self.home_region
    }

    #[must_use]
    pub fn failover_order(&self) -> &[RegionId] {
        &self.failover_order
    }

    #[must_use]
    pub fn region_ids(&self) -> impl Iterator<Item = &RegionId> {
        self.endpoints.keys()
    }

    #[must_use]
    pub fn endpoint(&self, region: &RegionId) -> Option<&RegionEndpoint> {
        self.endpoints.get(region)
    }

    /// Home region first, then configured failover order (ADR 0016 deterministic walk).
    #[must_use]
    pub fn route_candidates(&self) -> Vec<RegionId> {
        let mut out = Vec::with_capacity(1 + self.failover_order.len());
        out.push(self.home_region.clone());
        out.extend(self.failover_order.iter().cloned());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(url: &str) -> RegionEndpoint {
        RegionEndpoint {
            nats_url: url.to_string(),
            creds_ref: None,
        }
    }

    #[test]
    fn route_candidates_home_then_failover() {
        let home = RegionId::new("us-east").unwrap();
        let west = RegionId::new("us-west").unwrap();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(home.clone(), endpoint("nats://east"));
        endpoints.insert(west.clone(), endpoint("nats://west"));
        let topo = RegionTopology::new(home.clone(), vec![west.clone()], endpoints).unwrap();
        let candidates: Vec<_> = topo.route_candidates().into_iter().map(|r| r.as_str().to_string()).collect();
        assert_eq!(candidates, vec!["us-east", "us-west"]);
    }

    #[test]
    fn rejects_failover_containing_home() {
        let home = RegionId::new("us-east").unwrap();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(home.clone(), endpoint("nats://east"));
        let err = RegionTopology::new(home.clone(), vec![home], endpoints).unwrap_err();
        assert_eq!(err, TopologyBuildError::FailoverIncludesHome);
    }
}
