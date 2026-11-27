use async_trait::async_trait;
use trogon_sandbox_contract::ShortLivedToken;

use crate::{AcaError, ExecuteResult, SessionDeleteOutcome, SessionIdentifier, SessionInfo};

/// The Dynamic Sessions data plane operations the worker relies on.
#[async_trait]
pub trait AcaSessionApi: Send + Sync {
    async fn get_session(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
    ) -> Result<SessionInfo, AcaError>;

    async fn execute_code(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
        code: &str,
        language: &str,
    ) -> Result<ExecuteResult, AcaError>;

    async fn upload_file(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
        remote_path: &str,
        data: &[u8],
    ) -> Result<(), AcaError>;

    /// Deletes the session, reporting only a deletion the service confirmed or the absence of the
    /// session. A deletion still pending is an error.
    async fn delete_session(
        &self,
        bearer: &ShortLivedToken,
        identifier: &SessionIdentifier,
    ) -> Result<SessionDeleteOutcome, AcaError>;
}
