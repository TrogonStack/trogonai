use oci_client::Reference;
use tracing::instrument;

use crate::client::metadata_deadline;
use crate::{OciClient, OciError, RegistryHost};

impl OciClient {
    /// Tags of `repository`, paging through the registry's tag list.
    ///
    /// A repository with no pushes has no tag list and yields an empty vector.
    #[instrument(skip(self))]
    pub async fn list_tags(&self, registry: &RegistryHost, repository: &str) -> Result<Vec<String>, OciError> {
        let reference: Reference = format!("{registry}/{repository}:latest")
            .parse()
            .map_err(|error| OciError::InvalidReference(format!("{error}")))?;
        let auth = self.auth().auth_for_registry(reference.resolve_registry());
        let mut transport = self.transport(&reference, auth, metadata_deadline())?;
        match transport.tags().await {
            Ok(tags) => Ok(tags),
            Err(OciError::NotFound(_)) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }
}
