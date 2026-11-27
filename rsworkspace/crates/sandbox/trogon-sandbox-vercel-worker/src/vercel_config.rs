use crate::{VercelEndpoint, VercelProjectId, VercelTeamId, VercelToken};

/// Where the worker reaches Vercel and as whom. Every part is already checked, and the token is not part of
/// `Debug`.
#[derive(Debug, Clone)]
pub struct VercelConfig {
    endpoint: VercelEndpoint,
    team_id: VercelTeamId,
    project_id: VercelProjectId,
    token: VercelToken,
}

impl VercelConfig {
    pub fn new(
        endpoint: VercelEndpoint,
        team_id: VercelTeamId,
        project_id: VercelProjectId,
        token: VercelToken,
    ) -> Self {
        Self {
            endpoint,
            team_id,
            project_id,
            token,
        }
    }

    pub fn endpoint(&self) -> &VercelEndpoint {
        &self.endpoint
    }

    pub fn team_id(&self) -> &VercelTeamId {
        &self.team_id
    }

    pub fn project_id(&self) -> &VercelProjectId {
        &self.project_id
    }

    pub(crate) fn token(&self) -> &VercelToken {
        &self.token
    }
}
