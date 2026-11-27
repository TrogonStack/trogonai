use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{LeaseRequest, Port};

use super::assignments_error::AssignmentsError;
use super::constants::MAX_REGION_BYTES;
use crate::api::MAX_TIMEOUT_SECS;

/// What a restarted worker needs to adopt a native sandbox again.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recovery {
    pub exposed_ports: Vec<Port>,
    pub ttl_seconds: u32,
    pub region: Option<String>,
}

impl Recovery {
    pub(crate) fn from_request(request: &LeaseRequest) -> Self {
        Self {
            exposed_ports: request.spec.exposed_ports().to_vec(),
            ttl_seconds: request.ttl_seconds.clamp(1, MAX_TIMEOUT_SECS),
            region: request.region.clone(),
        }
    }

    pub(super) fn validate(&self) -> Result<(), AssignmentsError> {
        if !(1..=MAX_TIMEOUT_SECS).contains(&self.ttl_seconds) {
            return Err(AssignmentsError::InvalidLifetime);
        }
        if let Some(region) = &self.region
            && (region.is_empty() || region.len() > MAX_REGION_BYTES || region.chars().any(char::is_control))
        {
            return Err(AssignmentsError::InvalidRegion);
        }
        Ok(())
    }
}
