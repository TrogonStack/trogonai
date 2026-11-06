use super::*;
use crate::tenant_admission::{
    FileTenantAdmission, TenantAdmissionDecision, TenantAdmissionError, TenantAdmissionRequest,
};
use std::sync::Mutex;
use std::time::SystemTime;

#[tokio::test]
async fn removing_live_membership_prevents_the_next_dispatch_mint() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memberships.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "version": 1, "memberships": [{
                "account": tenant.account.as_str(), "identity": {"type": "principal", "principal_kind": "person", "principal_id": "usrstub"}
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = config_with(oidc_stub(), None, None, &[&tenant]);
    config.tenant_admission = Arc::new(FileTenantAdmission::new(&path));
    let dispatcher = CalloutDispatcher::new(config);
    assert!(dispatcher.dispatch(tenant.request(&server, |_| {})).await.is_ok());
    std::fs::write(&path, br#"{"version":1,"memberships":[]}"#).unwrap();
    assert!(matches!(
        dispatcher.dispatch(tenant.request(&server, |_| {})).await,
        Err(AuthCalloutError::TenantAdmission(TenantAdmissionError::NotMember))
    ));
    std::fs::write(&path, b"invalid").unwrap();
    assert!(matches!(
        dispatcher.dispatch(tenant.request(&server, |_| {})).await,
        Err(AuthCalloutError::TenantAdmission(TenantAdmissionError::Json(_)))
    ));
    std::fs::remove_file(path).unwrap();
    assert!(matches!(
        dispatcher.dispatch(tenant.request(&server, |_| {})).await,
        Err(AuthCalloutError::TenantAdmission(TenantAdmissionError::Read(_)))
    ));
}

#[tokio::test]
async fn a_registered_legacy_api_key_still_requires_explicit_membership() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let verifier = api_key_verifier_for(AudienceAccount::new(tenant.account.as_str()));
    let mut config = config_with(None, None, verifier, &[&tenant]);
    config.tenant_admission = Arc::new(TenantMemberships::new([]).unwrap());
    let dispatcher = CalloutDispatcher::new(config);
    let request = tenant.request(&server, |claims| {
        claims.nats.connect_opts.auth_token = Some("k_live_demo".into());
    });
    assert!(matches!(
        dispatcher.dispatch(request).await,
        Err(AuthCalloutError::TenantAdmission(TenantAdmissionError::NotMember))
    ));
}

#[derive(Clone)]
struct Clock(Arc<Mutex<SystemTime>>);

impl EpochClock for Clock {
    fn system_time(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

struct ExpiringAdmission {
    clock: Clock,
}

#[async_trait::async_trait]
impl TenantAdmission for ExpiringAdmission {
    async fn authorize(&self, _: TenantAdmissionRequest<'_>) -> Result<TenantAdmissionDecision, TenantAdmissionError> {
        tokio::task::yield_now().await;
        *self.clock.0.lock().unwrap() = SystemTime::now() + Duration::from_secs(7200);
        Ok(TenantAdmissionDecision::Permit)
    }
}

#[tokio::test]
async fn membership_work_cannot_extend_source_validity_before_mint() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let clock = Clock(Arc::new(Mutex::new(SystemTime::now())));
    let mut config = config_with(oidc_stub(), None, None, &[&tenant]);
    config.tenant_admission = Arc::new(ExpiringAdmission { clock: clock.clone() });
    let dispatcher = CalloutDispatcher::new(config).with_clock(clock);
    assert!(matches!(
        dispatcher.dispatch(tenant.request(&server, |_| {})).await,
        Err(AuthCalloutError::CredentialVerification(CredentialError::Validity(
            crate::CredentialExpiryError::Expired
        )))
    ));
}
