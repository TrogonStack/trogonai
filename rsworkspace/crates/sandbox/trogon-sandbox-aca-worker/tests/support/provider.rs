use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use tempfile::TempDir;
use trogon_sandbox_aca_worker::{AcaWorkerProvider, ProviderConfig};
use trogon_sandbox_contract::{LifecycleEvent, OciImageRef, PullSecretId, ShortLivedToken};
use trogon_sandbox_protocol::LifecycleEmitter;
use trogon_sandbox_volumes::StoreResourceName;
use url::Url;

use super::fake_aca::FakeAca;
use super::fake_issuer::FakeIssuer;
use super::local_backends::LocalBackends;

/// Records every lifecycle event a provider emits.
#[derive(Default)]
pub struct CapturingEmitter {
    events: Mutex<Vec<LifecycleEvent>>,
}

impl CapturingEmitter {
    pub fn events(&self) -> Vec<LifecycleEvent> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl LifecycleEmitter for CapturingEmitter {
    async fn emit(&self, event: LifecycleEvent) {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

pub fn config(login: &str) -> Result<ProviderConfig, Box<dyn std::error::Error>> {
    Ok(ProviderConfig {
        management_endpoint: Url::parse("http://pool.invalid/management")?,
        login_endpoint: Url::parse(login)?,
        session_pool_name: "pool".to_owned(),
        default_image: Some(OciImageRef::parse_unqualified("registry.example/default:2")),
        azure_credential_id: PullSecretId::new("azure-sp"),
        volume_credential_id: PullSecretId::new("azure-volume"),
        volume_storage_account: Some(StoreResourceName::new("account")?),
    })
}

/// A provider wired to in-process fakes, plus handles on each of them.
pub struct Fixture {
    pub provider: Arc<AcaWorkerProvider>,
    pub aca: Arc<FakeAca>,
    pub issuer: Arc<FakeIssuer>,
    pub emitter: Arc<CapturingEmitter>,
    pub volumes: TempDir,
}

impl Fixture {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::build(config("http://login.invalid")?, true, None)
    }

    pub fn with_config(config: ProviderConfig) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build(config, true, None)
    }

    /// A fixture whose volume backend cannot be opened, because its root lies beneath a file.
    pub fn with_unusable_volumes() -> Result<Self, Box<dyn std::error::Error>> {
        let file = tempfile::NamedTempFile::new()?;
        let root = file.path().join("volumes");
        let fixture = Self::build(config("http://login.invalid")?, true, Some(root))?;
        file.keep()?;
        Ok(fixture)
    }

    /// A fixture whose provider exchanges the service principal instead of bypassing it.
    pub fn exchanging(login: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::build(config(login)?, false, None)
    }

    fn build(
        config: ProviderConfig,
        bypass: bool,
        volume_root: Option<PathBuf>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let aca = Arc::new(FakeAca::default());
        let issuer = Arc::new(FakeIssuer::new());
        let emitter = Arc::new(CapturingEmitter::default());
        let volumes = tempfile::tempdir()?;
        let root = volume_root.unwrap_or_else(|| volumes.path().to_path_buf());
        let mut provider = AcaWorkerProvider::with_emitter(emitter.clone(), config, issuer.clone())
            .with_client(aca.clone())
            .with_volume_backends(Arc::new(LocalBackends { root }));
        if bypass {
            provider = provider.with_auth_bypass(ShortLivedToken::new("bypass"));
        }
        Ok(Self {
            provider: Arc::new(provider),
            aca,
            issuer,
            emitter,
            volumes,
        })
    }

    pub fn volume_root(&self) -> PathBuf {
        self.volumes.path().to_path_buf()
    }
}
