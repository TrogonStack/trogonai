use super::*;
use crate::tenant_admission::{
    FileTenantAdmission, TenantAdmissionDecision, TenantAdmissionError, TenantAdmissionRequest,
};
use std::error::Error;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

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

#[tokio::test]
async fn tagged_api_key_enrollment_does_not_accept_other_sources_or_override_its_registered_account() {
    let tenant = Tenant::generate();
    let other = Tenant::generate();
    let server = FixtureServer::generate();
    let verifier = api_key_verifier_for(AudienceAccount::new(tenant.account.as_str()));
    for (identity, expected) in [
        (
            json!({"type":"legacy_api_key", "subject":"svc-demo"}),
            TenantAdmissionDecision::Permit,
        ),
        (
            json!({"type":"legacy_mtls", "subject":"svc-demo"}),
            TenantAdmissionDecision::Deny,
        ),
        (
            json!({"type":"oidc", "issuer":"https://issuer.example", "subject":"svc-demo"}),
            TenantAdmissionDecision::Deny,
        ),
    ] {
        let document = json!({"version":1, "memberships":[{
            "account":tenant.account.as_str(), "identity":identity
        }]});
        let mut config = config_with(None, None, verifier.clone(), &[&tenant, &other]);
        config.tenant_admission =
            Arc::new(TenantMemberships::from_json(&serde_json::to_vec(&document).unwrap()).unwrap());
        let dispatcher = CalloutDispatcher::new(config);
        let request = |tenant: &Tenant| {
            tenant.request(&server, |claims| {
                claims.nats.connect_opts.auth_token = Some("k_live_demo".into());
            })
        };
        let result = dispatcher.dispatch(request(&tenant)).await;
        if expected == TenantAdmissionDecision::Permit {
            let minted = result.unwrap();
            let payload = decode_nats_user_payload(minted.as_str()).unwrap();
            assert_eq!(payload["aud"], tenant.account.as_str());
            assert_eq!(payload["principal_kind"], "service");
        } else {
            assert!(matches!(
                result,
                Err(AuthCalloutError::TenantAdmission(TenantAdmissionError::NotMember))
            ));
        }
        assert!(matches!(
            dispatcher.dispatch(request(&other)).await,
            Err(AuthCalloutError::CredentialVerification(
                CredentialError::InvalidRequest(_)
            ))
        ));
    }
}

fn assert_opaque_validity_denial(
    error: &AuthCalloutError,
    request: &ServerAuthRequestClaims,
    server: &FixtureServer,
    tenant: &Tenant,
    expected: crate::DenialCategory,
) {
    let category = crate::DenialCategory::from_auth_callout_error(error);
    assert_eq!(category, expected);
    let codec = crate::AuthCalloutWireCodec::new(server.auth_account().clone(), tenant.signing_key.clone(), None);
    let encoded = codec.encode_denial(request, category.as_str()).unwrap();
    let encoded = std::str::from_utf8(&encoded).unwrap();
    let payload = decode_nats_user_payload(encoded).unwrap();
    assert_eq!(payload["nats"]["error"], expected.as_str());
    assert!(payload["nats"]["jwt"].as_str().is_none_or(str::is_empty));
}

#[tokio::test]
async fn a_zero_or_subsecond_configured_broker_lifetime_cannot_mint_a_rounded_up_credential() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    for maximum in [Duration::ZERO, Duration::from_nanos(999_999_999)] {
        let mut config = config_with(oidc_stub(), None, None, &[&tenant]);
        config.user_jwt_ttl = maximum;
        let dispatcher = CalloutDispatcher::new(config);
        let request = tenant.request(&server, |_| {});
        let error = dispatcher.dispatch(request.clone()).await.unwrap_err();
        assert!(matches!(
            &error,
            AuthCalloutError::CredentialVerification(CredentialError::Validity(
                crate::CredentialExpiryError::EmptyLifetime
            ))
        ));
        assert_opaque_validity_denial(&error, &request, &server, &tenant, crate::DenialCategory::InternalError);
    }
    for maximum in [Duration::from_secs(1), Duration::from_millis(1500)] {
        let mut config = config_with(oidc_stub(), None, None, &[&tenant]);
        config.user_jwt_ttl = maximum;
        let dispatcher = CalloutDispatcher::new(config);
        let minted = dispatcher.dispatch(tenant.request(&server, |_| {})).await.unwrap();
        let payload = decode_nats_user_payload(minted.as_str()).unwrap();
        assert_eq!(payload["exp"].as_i64().unwrap() - payload["iat"].as_i64().unwrap(), 1);
    }
}

#[tokio::test]
async fn legacy_api_keys_cannot_round_a_zero_or_subsecond_configured_lifetime_up_to_one_second() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let mut unexpected_mints = Vec::new();
    for maximum in [Duration::ZERO, Duration::from_nanos(999_999_999)] {
        let verifier = api_key_verifier_for(AudienceAccount::new(tenant.account.as_str()));
        let mut config = config_with(None, None, verifier, &[&tenant]);
        config.user_jwt_ttl = maximum;
        let dispatcher = CalloutDispatcher::new(config);
        let request = tenant.request(&server, |claims| {
            claims.nats.connect_opts.auth_token = Some("k_live_demo".into());
        });
        match dispatcher.dispatch(request.clone()).await {
            Ok(_) => unexpected_mints.push(maximum),
            Err(error) => {
                assert!(matches!(
                    &error,
                    AuthCalloutError::Jwt(crate::jwt::JwtError::Lifetime(
                        crate::CredentialExpiryError::EmptyLifetime
                    ))
                ));
                assert_opaque_validity_denial(&error, &request, &server, &tenant, crate::DenialCategory::InternalError);
            }
        }
    }
    assert!(
        unexpected_mints.is_empty(),
        "legacy admission exceeded its configured maximum: {unexpected_mints:?}"
    );
}

struct SourceExpiryOidcVerifier {
    seconds: i64,
}

#[async_trait::async_trait]
impl OidcVerifier for SourceExpiryOidcVerifier {
    async fn verify(
        &self,
        token: &BearerToken,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError> {
        let verifier = StubOidcVerifier {
            sub: "user-1",
            caller: CallerId::new("usrstub").unwrap(),
        };
        let verified = verifier.verify(token, account).await?;
        let expiry = crate::CredentialExpiry::from_unix_seconds(self.seconds).map_err(CredentialError::Validity)?;
        Ok(VerifiedCredential::new(verified.claims().clone(), expiry))
    }
}

#[tokio::test]
async fn invalid_source_timestamps_are_refused_with_an_opaque_credential_denial() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    for seconds in [i64::MIN, -1, 0] {
        let verifier = Arc::new(SourceExpiryOidcVerifier { seconds });
        let dispatcher = CalloutDispatcher::new(config_with(Some(verifier), None, None, &[&tenant]));
        let request = tenant.request(&server, |_| {});
        let error = dispatcher.dispatch(request.clone()).await.unwrap_err();
        assert!(matches!(
            &error,
            AuthCalloutError::CredentialVerification(CredentialError::Validity(
                crate::CredentialExpiryError::InvalidTimestamp
            ))
        ));
        assert_opaque_validity_denial(
            &error,
            &request,
            &server,
            &tenant,
            crate::DenialCategory::InvalidCredentials,
        );
    }
}

#[tokio::test]
async fn a_before_epoch_dispatch_clock_fails_closed_without_disclosing_clock_details() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let clock = Clock(Arc::new(Mutex::new(UNIX_EPOCH - Duration::from_secs(1))));
    let dispatcher = CalloutDispatcher::new(config_with(oidc_stub(), None, None, &[&tenant])).with_clock(clock);
    let request = tenant.request(&server, |_| {});
    let error = dispatcher.dispatch(request.clone()).await.unwrap_err();
    let AuthCalloutError::CredentialVerification(CredentialError::Validity(validity)) = &error else {
        panic!("expected a typed credential-validity failure");
    };
    assert!(matches!(validity, crate::CredentialExpiryError::ClockBeforeEpoch(_)));
    assert!(
        validity
            .source()
            .unwrap()
            .downcast_ref::<std::time::SystemTimeError>()
            .is_some()
    );
    assert_opaque_validity_denial(&error, &request, &server, &tenant, crate::DenialCategory::InternalError);
}

#[tokio::test]
async fn carried_source_expiry_is_an_exclusive_mint_deadline_even_with_a_longer_broker_ttl() {
    let tenant = Tenant::generate();
    let server = FixtureServer::generate();
    let verifier = Arc::new(SourceExpiryOidcVerifier { seconds: 1_000_010 });
    let evidence = verifier
        .verify(
            &BearerToken::new("fixture-source"),
            &AudienceAccount::new(tenant.account.as_str()),
        )
        .await
        .unwrap();
    let deadline = u64::try_from(evidence.expires_at().unix_seconds()).unwrap();
    let clock = Clock(Arc::new(Mutex::new(UNIX_EPOCH + Duration::from_secs(deadline - 1))));
    let dispatcher =
        CalloutDispatcher::new(config_with(Some(verifier), None, None, &[&tenant])).with_clock(clock.clone());
    let minted = dispatcher.dispatch(tenant.request(&server, |_| {})).await.unwrap();
    let payload = decode_nats_user_payload(minted.as_str()).unwrap();
    assert_eq!(payload["exp"].as_u64().unwrap(), deadline);
    assert_eq!(payload["iat"].as_u64().unwrap(), deadline - 1);
    *clock.0.lock().unwrap() = UNIX_EPOCH + Duration::from_secs(deadline);
    let request = tenant.request(&server, |_| {});
    let error = dispatcher.dispatch(request.clone()).await.unwrap_err();
    assert!(matches!(
        &error,
        AuthCalloutError::CredentialVerification(CredentialError::Validity(crate::CredentialExpiryError::Expired))
    ));
    assert_opaque_validity_denial(
        &error,
        &request,
        &server,
        &tenant,
        crate::DenialCategory::InvalidCredentials,
    );
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
