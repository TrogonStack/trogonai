//! A lease against a real Dynamic Sessions pool: the session is created, ready and deleted again.
//!
//! Skipped unless `SANDBOX_ACA_LIVE_TEST=1`. The worker's own variables select the pool
//! (`SANDBOX_ACA_POOL_MANAGEMENT_ENDPOINT`, `SANDBOX_ACA_SESSION_POOL_NAME`, `SANDBOX_ACA_LOGIN_ENDPOINT`), and
//! `SANDBOX_ACA_LIVE_SERVICE_PRINCIPAL` holds the service principal as `client_id:client_secret:tenant_id`.

mod support;

use std::num::NonZeroU32;
use std::sync::Arc;

use async_trait::async_trait;
use clap::Parser;
use support::lease::Lease;
use support::{TestResult, provider::CapturingEmitter};
use trogon_sandbox_aca_worker::{AcaError, AcaWorkerProvider, CredentialIssuer, WorkerArgs};
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, LeaseId, PullSecretId, ShortLivedToken, TenantId};
use trogon_sandbox_protocol::Provider;
use trogon_std::env::{ReadEnv, SystemEnv};

const GATE: &str = "SANDBOX_ACA_LIVE_TEST";
const PRINCIPAL: &str = "SANDBOX_ACA_LIVE_SERVICE_PRINCIPAL";

/// Hands every credential request the service principal from the environment.
struct EnvIssuer {
    principal: ShortLivedToken,
}

#[async_trait]
impl CredentialIssuer for EnvIssuer {
    async fn issue(
        &self,
        _provider: CredentialProvider,
        _tenant: &TenantId,
        _pull_secret_id: &PullSecretId,
        _scope: CredentialScope,
        _ttl_seconds: NonZeroU32,
    ) -> Result<ShortLivedToken, AcaError> {
        Ok(self.principal.clone())
    }
}

#[tokio::test]
#[ignore = "requires an Azure Dynamic Sessions pool and a service principal; set SANDBOX_ACA_LIVE_TEST=1 and use --ignored"]
async fn a_lease_creates_a_session_in_a_real_pool_and_cancel_deletes_it() -> TestResult {
    if SystemEnv.var(GATE).as_deref() != Ok("1") {
        eprintln!("skipped: set {GATE}=1 to run against a real pool");
        return Ok(());
    }
    let args = WorkerArgs::try_parse_from(["live"])?;
    let issuer = Arc::new(EnvIssuer {
        principal: ShortLivedToken::new(SystemEnv.var(PRINCIPAL)?),
    });
    let provider =
        AcaWorkerProvider::with_emitter(Arc::new(CapturingEmitter::default()), args.provider_config(), issuer);

    let lease_id = LeaseId::new("live-aca-smoke");
    let granted = provider.lease(&Lease::empty().build()?, &lease_id).await?;
    assert!(!granted.sandbox_id.as_str().is_empty());
    assert_eq!(provider.active_lease_count().await, 1);

    provider.cancel(&lease_id).await?;
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}
