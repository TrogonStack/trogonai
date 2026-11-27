use trogon_sandbox_contract::{LeaseError, SshAccess, SshAccessResponse};

use crate::DaytonaWorkerProvider;
use crate::constants::DEFAULT_SSH_ACCESS_MINUTES;

impl DaytonaWorkerProvider {
    /// Mints SSH access through Daytona's own SSH gateway. The access token is the SSH user and expires after
    /// `ttl_seconds`, rounded up to whole minutes (an hour when unset).
    pub async fn ssh_access_for_sandbox(&self, sandbox_id: &str, ttl_seconds: Option<u32>) -> SshAccessResponse {
        let minutes = ttl_seconds.map_or(DEFAULT_SSH_ACCESS_MINUTES, |seconds| seconds.div_ceil(60).max(1));
        match self.api.ssh_access(sandbox_id, minutes).await {
            Ok(grant) => SshAccessResponse::Available(SshAccess {
                host: grant.host,
                port: grant.port,
                user: grant.token,
                command: Some(grant.command),
                expires_at: grant.expires_at,
            }),
            Err(error) => SshAccessResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            }),
        }
    }
}
