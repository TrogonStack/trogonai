use std::fmt;

use super::region_id::RegionId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionRouteError {
    pub kind: RegionRouteErrorKind,
    pub attempted: Vec<RegionId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionRouteErrorKind {
    AllRegionsUnreachable,
}

impl RegionRouteError {
    #[must_use]
    pub fn all_regions_unreachable(attempted: Vec<RegionId>) -> Self {
        Self {
            kind: RegionRouteErrorKind::AllRegionsUnreachable,
            attempted,
        }
    }
}

impl fmt::Display for RegionRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            RegionRouteErrorKind::AllRegionsUnreachable => {
                write!(
                    f,
                    "all regions unreachable (tried: {})",
                    self.attempted
                        .iter()
                        .map(RegionId::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }
}

impl std::error::Error for RegionRouteError {}
