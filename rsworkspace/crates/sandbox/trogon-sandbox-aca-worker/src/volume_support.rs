use std::path::Path;
use std::sync::Arc;

use tracing::debug;
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, PullSecretId, TenantId};
use trogon_sandbox_volumes::{
    StoreResourceName, VolumeClient, VolumeClientConfig, VolumeMode, VolumePrefix, build_changed_manifest,
};

use crate::constants::VOLUME_CREDENTIAL_TTL_SECONDS;
use crate::{AcaError, CredentialIssuer, VolumeBackends, VolumeCredential, VolumeSyncPlan};

/// What the worker needs to pull a volume into a workspace and push its changes back.
#[derive(Clone)]
pub struct VolumeSupport {
    pub issuer: Arc<dyn CredentialIssuer>,
    pub backends: Arc<dyn VolumeBackends>,
    pub storage_account: Option<StoreResourceName>,
    pub credential_id: PullSecretId,
}

impl VolumeSupport {
    /// A client for the volume, after the broker issued a credential scoped to it.
    pub async fn client(
        &self,
        tenant: &TenantId,
        container: &StoreResourceName,
        account: &StoreResourceName,
        prefix: &VolumePrefix,
        mode: VolumeMode,
    ) -> Result<VolumeClient, AcaError> {
        let (bucket, prefix_label) = (container.as_str().to_owned(), prefix.as_str().to_owned());
        let scope = match mode {
            VolumeMode::ReadOnly => CredentialScope::S3ReadOnly {
                bucket,
                prefix: prefix_label,
            },
            VolumeMode::ReadWrite | VolumeMode::CopyOnWrite => CredentialScope::S3ReadWrite {
                bucket,
                prefix: prefix_label,
            },
        };
        let token = self
            .issuer
            .issue(
                CredentialProvider::S3,
                tenant,
                &self.credential_id,
                scope,
                VOLUME_CREDENTIAL_TTL_SECONDS,
            )
            .await?;
        let credential = VolumeCredential::parse(&token)?;
        debug!(account = %credential.account, "issued volume credential");
        let backend = self.backends.backend(container.clone(), account.clone(), &credential);
        Ok(VolumeClient::with_mode(
            VolumeClientConfig {
                backend,
                kms_key_id: None,
            },
            mode,
        )?)
    }

    /// Pushes the files that changed in `local_path` back to the volume.
    pub async fn push(&self, plan: &VolumeSyncPlan, tenant: &TenantId, local_path: &Path) -> Result<(), AcaError> {
        let client = self
            .client(tenant, &plan.container, &plan.account, &plan.prefix, plan.mode)
            .await?;
        let manifest = build_changed_manifest(local_path, None)?;
        client.push_changed(&plan.prefix, local_path, manifest).await?;
        Ok(())
    }
}
