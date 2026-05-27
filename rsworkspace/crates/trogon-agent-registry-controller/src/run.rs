use std::error::Error;

use tracing::info;

use crate::audit::AuditPublisher;
use crate::config::ControllerConfig;
use crate::kv::{ControllerStore, connect_nats};
use crate::signer::{load_signer, verifying_key_from_pem, ManifestSigner, SignerKind};
use crate::sync::{GitSyncConfig, SyncEngine, SyncError, SyncReport};

type BoxError = Box<dyn Error + Send + Sync>;

pub async fn run_controller(config: ControllerConfig) -> Result<(), BoxError> {
    info!(
        repo = %config.repo_path.display(),
        signer = ?config.signer_source,
        interval_secs = config.sync_interval_secs,
        "starting trogon-agent-registry-controller"
    );

    let signer = load_signer(config.signer_source, &config.signer_key_path)?;
    let client = connect_nats(&config.nats_url).await?;
    let jetstream = async_nats::jetstream::new(client.clone());
    let store = ControllerStore::open(&jetstream, config.auto_create_bucket).await?;
    let audit = AuditPublisher::new(client, "registry-controller");

    let git_config = GitSyncConfig {
        repo_path: config.repo_path.clone(),
        agents_dir: config.agents_dir.clone(),
        git_remote: config.git_remote.clone(),
        git_ref: config.git_ref.clone(),
    };

    let engine = SyncEngine {
        signer: signer.as_ref(),
        store: Some(&store),
        audit: Some(&audit),
    };

    let mut shutdown = Box::pin(trogon_std::signal::shutdown_signal());
    loop {
        match engine.sync_repo(&git_config, false).await {
            Ok(report) => info!(
                git_commit = %report.git_commit,
                seen = report.manifests_seen,
                applied = report.manifests_applied,
                "registry sync complete"
            ),
            Err(error) => tracing::error!(%error, "registry sync failed"),
        }

        tokio::select! {
            _ = &mut shutdown => break,
            _ = tokio::time::sleep(config.sync_interval()) => {}
        }
    }

    info!("trogon-agent-registry-controller stopped");
    Ok(())
}

pub fn validate_repo_sync(
    config: GitSyncConfig,
    verify_key_pem: &str,
    signer_key_path: Option<&std::path::Path>,
) -> Result<SyncReport, SyncError> {
    let verifying_key = verifying_key_from_pem(verify_key_pem).map_err(|error| SyncError::Signer(error.to_string()))?;
    let signer: Box<dyn ManifestSigner> = if let Some(path) = signer_key_path {
        load_signer(SignerKind::File, path).map_err(|error| SyncError::Signer(error.to_string()))?
    } else if let Ok(file_signer) = crate::signer::FileManifestSigner::from_pem(verify_key_pem) {
        Box::new(file_signer)
    } else {
        Box::new(VerifyOnlySigner { verifying_key })
    };

    let engine = SyncEngine {
        signer: signer.as_ref(),
        store: None,
        audit: None,
    };

    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(engine.sync_repo(&config, true))
}

struct VerifyOnlySigner {
    verifying_key: ed25519_dalek::VerifyingKey,
}

impl ManifestSigner for VerifyOnlySigner {
    fn key_id(&self) -> &str {
        "verify-only"
    }

    fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.verifying_key
    }

    fn sign_payload(
        &self,
        _payload: &[u8],
    ) -> Result<crate::manifest::ManifestSignature, crate::signer::SignerError> {
        Err(crate::signer::SignerError::Unsupported(
            "verify-only signer cannot sign manifests".into(),
        ))
    }
}
