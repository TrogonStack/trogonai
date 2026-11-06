use crate::tenant_admission::{
    TenantAdmission, TenantAdmissionDecision, TenantAdmissionError, TenantAdmissionRequest, TenantMemberships,
};
use crate::tenant_memberships::MAX_REGISTRY_BYTES;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

const MAX_CONCURRENT_READS: usize = 16;
const READ_TIMEOUT: Duration = Duration::from_secs(1);
pub const ENV_TENANT_MEMBERSHIPS: &str = "AUTH_CALLOUT_TENANT_MEMBERSHIPS";

pub struct FileTenantAdmission {
    path: PathBuf,
    reads: Arc<Semaphore>,
}

impl FileTenantAdmission {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            reads: Arc::new(Semaphore::new(MAX_CONCURRENT_READS)),
        }
    }

    async fn memberships(&self) -> Result<TenantMemberships, TenantAdmissionError> {
        let permit = Arc::clone(&self.reads)
            .try_acquire_owned()
            .map_err(|_| TenantAdmissionError::Busy)?;
        let path = self.path.clone();
        let work = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NONBLOCK);
            }
            #[cfg(not(unix))]
            if !std::fs::metadata(&path).map_err(TenantAdmissionError::Read)?.is_file() {
                return Err(TenantAdmissionError::NotRegularFile);
            }
            let mut file = options.open(path).map_err(TenantAdmissionError::Read)?;
            if !file.metadata().map_err(TenantAdmissionError::Read)?.is_file() {
                return Err(TenantAdmissionError::NotRegularFile);
            }
            let mut bytes = Vec::new();
            file.by_ref()
                .take((MAX_REGISTRY_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(TenantAdmissionError::Read)?;
            TenantMemberships::from_json(&bytes)
        });
        tokio::time::timeout(READ_TIMEOUT, work)
            .await
            .map_err(|_| TenantAdmissionError::Timeout)?
            .map_err(TenantAdmissionError::Worker)?
    }

    pub async fn validate(&self) -> Result<(), TenantAdmissionError> {
        self.memberships().await.map(|_| ())
    }
}

#[async_trait::async_trait]
impl TenantAdmission for FileTenantAdmission {
    async fn authorize(
        &self,
        request: TenantAdmissionRequest<'_>,
    ) -> Result<TenantAdmissionDecision, TenantAdmissionError> {
        Ok(self.memberships().await?.decision(request))
    }
}

#[cfg(test)]
mod tests;
