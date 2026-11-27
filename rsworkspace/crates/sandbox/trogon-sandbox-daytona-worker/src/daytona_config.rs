use crate::{DaytonaApiKey, DaytonaEndpoint, DaytonaOrganizationId};

/// Where the worker reaches Daytona and as whom. Every part is already checked, and the key is not part of `Debug`.
#[derive(Debug, Clone)]
pub struct DaytonaConfig {
    endpoint: DaytonaEndpoint,
    api_key: DaytonaApiKey,
    organization: Option<DaytonaOrganizationId>,
}

impl DaytonaConfig {
    pub fn new(endpoint: DaytonaEndpoint, api_key: DaytonaApiKey, organization: Option<DaytonaOrganizationId>) -> Self {
        Self {
            endpoint,
            api_key,
            organization,
        }
    }

    pub fn endpoint(&self) -> &DaytonaEndpoint {
        &self.endpoint
    }

    pub fn organization(&self) -> Option<&DaytonaOrganizationId> {
        self.organization.as_ref()
    }

    pub(crate) fn api_key(&self) -> &DaytonaApiKey {
        &self.api_key
    }
}
