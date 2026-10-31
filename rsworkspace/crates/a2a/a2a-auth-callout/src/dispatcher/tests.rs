use super::*;
use crate::bridge_mint::{BridgeClientInfo, BridgeMintRequest};
use crate::credentials::api_key::{ApiKey, ApiKeyEntry, ApiKeyRegistry, HmacApiKeyVerifier};
use crate::credentials::mtls::ClientCertPem;
use crate::credentials::oidc::BearerToken;
use crate::error::CredentialError;
use crate::jwt::{
    AudienceAccount, CallerId, ExternalSubject, SpiceDbPrincipal, UserJwtClaims, decode_nats_user_payload,
};
use crate::permissions::IssuedPermissions;
use crate::wire::test_encode::{FixtureServer, verified_client_tls};
use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use nkeys::KeyPair;
use serde_json::json;
use trogon_identity_types::PrincipalKind;

const FIXTURE_CLIENT_CERT: &str = "-----BEGIN CERT-----\n-----END CERT-----";

fn encode_fixture_request(
    server: &FixtureServer,
    patch: impl FnMut(&mut Claims<AuthRequest>),
) -> ServerAuthRequestClaims {
    server.request(&KeyPair::new_user(), patch)
}

#[derive(Clone)]
struct Tenant {
    account: TenantAccount,
    signing_key: AccountSigningKey,
}

impl Tenant {
    fn generate() -> Self {
        let account = TenantAccount::parse(&KeyPair::new_account().public_key()).expect("tenant account");
        let seed = KeyPair::new_account().seed().expect("signing key seed");
        let signing_key = AccountSigningKey::from_seed(&seed).expect("tenant signing key");
        Self { account, signing_key }
    }

    fn request(
        &self,
        server: &FixtureServer,
        mut patch: impl FnMut(&mut Claims<AuthRequest>),
    ) -> ServerAuthRequestClaims {
        let account = self.account.as_str().to_owned();
        encode_fixture_request(server, |claims| {
            claims.nats.connect_opts.user = Some(account.clone());
            patch(claims);
        })
    }

    fn bridge_request(&self) -> ServerAuthRequestClaims {
        ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
            user_nkey: None,
            user_jwt: None,
            account: Some(self.account.as_str().to_owned()),
            client_info: Some(BridgeClientInfo {
                client_cert_pem: Some(FIXTURE_CLIENT_CERT.to_owned()),
            }),
            connect_opts: None,
        })
        .unwrap()
    }
}

pub struct AlwaysDenyDispatcher;

#[async_trait::async_trait]
impl AuthDispatcher for AlwaysDenyDispatcher {
    async fn dispatch(&self, _request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        Err(crate::error::CredentialError::InvalidCredentials("stub: always deny".into()).into())
    }
}

#[tokio::test]
async fn always_deny_returns_err() {
    let server = FixtureServer::generate();
    let req = encode_fixture_request(&server, |_| {});
    assert!(AlwaysDenyDispatcher.dispatch(req).await.is_err());
}

struct StubOidcVerifier {
    sub: &'static str,
}

#[async_trait::async_trait]
impl OidcVerifier for StubOidcVerifier {
    async fn verify(&self, _token: &BearerToken, account: &AudienceAccount) -> Result<UserJwtClaims, AuthCalloutError> {
        let caller_id = CallerId::new("usrstub").unwrap();
        Ok(UserJwtClaims {
            kid: crate::signing_key_source::unminted_placeholder(),
            sub: ExternalSubject::new(self.sub).unwrap(),
            aud: account.clone(),
            data: SpiceDbPrincipal(json!({"spicedb_subject": self.sub})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
            principal_kind: PrincipalKind::Person,
        })
    }
}

struct StubMtlsVerifier;

#[async_trait::async_trait]
impl MTlsVerifier for StubMtlsVerifier {
    async fn verify(
        &self,
        _cert: &ClientCertPem,
        account: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError> {
        let caller_id = CallerId::new("mtlsstub").unwrap();
        Ok(UserJwtClaims {
            kid: crate::signing_key_source::unminted_placeholder(),
            sub: ExternalSubject::new("CN=svc").unwrap(),
            aud: account.clone(),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "svc"})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
            principal_kind: PrincipalKind::Service,
        })
    }
}

fn dispatcher_with(
    oidc: Option<Arc<dyn OidcVerifier>>,
    mtls: Option<Arc<dyn MTlsVerifier>>,
    api_key: Option<Arc<dyn ApiKeyVerifier>>,
    tenants: &[&Tenant],
) -> CalloutDispatcher {
    let tenant_signing_keys =
        TenantSigningKeys::new(tenants.iter().map(|t| (t.account.clone(), t.signing_key.clone()))).unwrap();
    CalloutDispatcher::new(CalloutDispatcherConfig {
        tenant_signing_keys,
        user_jwt_ttl: Duration::from_secs(60),
        oidc,
        mtls,
        api_key,
    })
}

fn oidc_stub() -> Option<Arc<dyn OidcVerifier>> {
    Some(Arc::new(StubOidcVerifier { sub: "user-1" }))
}

fn api_key_verifier_for(audience: AudienceAccount) -> Option<Arc<dyn ApiKeyVerifier>> {
    let mut registry = ApiKeyRegistry::new(b"hmac-test-secret".to_vec());
    let key = ApiKey::new("k_live_demo").unwrap();
    registry.register(
        &key,
        ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("svc/demo"),
            audience,
            external_subject: ExternalSubject::new("svc-demo").unwrap(),
        },
    );
    Some(Arc::new(HmacApiKeyVerifier::new(Arc::new(registry))))
}

#[tokio::test]
async fn dispatch_oidc_happy_path_mints_jwt() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&tenant]);
    let req = tenant.request(&server, |_| {});
    let resp = d.dispatch(req).await.unwrap();
    assert_eq!(resp.as_str().split('.').count(), 3);
}

#[tokio::test]
async fn dispatch_mints_a_user_the_tenant_signing_key_issues_on_behalf_of_the_tenant() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let other = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&other, &tenant]);
    let user = KeyPair::new_user();
    let user_public = user.public_key();
    let account = tenant.account.as_str().to_owned();
    let req = server.request(&user, |claims| {
        claims.nats.connect_opts.user = Some(account.clone());
    });

    let minted = d.dispatch(req).await.unwrap();

    let payload = decode_nats_user_payload(minted.as_str()).unwrap();
    assert_eq!(payload["iss"], tenant.signing_key.public_key());
    assert_eq!(payload["nats"]["issuer_account"], tenant.account.as_str());
    assert_eq!(payload["sub"], user_public);
    assert_eq!(payload["aud"], tenant.account.as_str());
    assert_eq!(payload["nats"]["pub"]["allow"], json!(["a2a.v1.gateway.>"]));
    assert_eq!(
        payload["nats"]["sub"]["allow"],
        json!(["_INBOX.usrstub.>", "a2a.v1.push.usrstub.>"])
    );
}

#[tokio::test]
async fn dispatch_rejects_a_tenant_account_without_a_signing_key() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let unlisted = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&tenant]);
    let req = unlisted.request(&server, |_| {});
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::UnknownAccount(requested))
            if requested == unlisted.account.as_str()
    ));
}

#[tokio::test]
async fn dispatch_rejects_unknown_account() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&tenant]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.user = Some("tenant-evil".into());
    });
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::UnknownAccount(requested))
            if requested == "tenant-evil"
    ));
}

#[tokio::test]
async fn dispatch_rejects_request_without_account() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&tenant]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.user = None;
    });
    assert!(d.dispatch(req).await.is_err());
}

#[tokio::test]
async fn dispatch_reads_auth_token_before_pass() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.pass = Some("k_live_demo".into());
    });
    assert!(d.dispatch(req).await.is_ok());
}

#[tokio::test]
async fn dispatch_verifies_a_secret_instead_of_the_client_certificate() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let mtls: Arc<dyn MTlsVerifier> = Arc::new(StubMtlsVerifier);
    let d = dispatcher_with(oidc_stub(), Some(mtls), None, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.client_tls = Some(verified_client_tls(&[FIXTURE_CLIENT_CERT]));
    });
    let minted = d.dispatch(req).await.unwrap();
    let payload = decode_nats_user_payload(minted.as_str()).unwrap();
    assert_eq!(
        payload["nats"]["sub"]["allow"],
        json!(["_INBOX.usrstub.>", "a2a.v1.push.usrstub.>"])
    );
}

#[tokio::test]
async fn dispatch_verifies_the_client_certificate_when_no_secret_is_presented() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let mtls: Arc<dyn MTlsVerifier> = Arc::new(StubMtlsVerifier);
    let d = dispatcher_with(oidc_stub(), Some(mtls), None, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.auth_token = None;
        c.nats.client_tls = Some(verified_client_tls(&[FIXTURE_CLIENT_CERT]));
    });
    let minted = d.dispatch(req).await.unwrap();
    let payload = decode_nats_user_payload(minted.as_str()).unwrap();
    assert_eq!(
        payload["nats"]["sub"]["allow"],
        json!(["_INBOX.mtlsstub.>", "a2a.v1.push.mtlsstub.>"])
    );
}

#[tokio::test]
async fn dispatch_mtls_when_cert_present() {
    let tenant = Tenant::generate();
    let mtls: Arc<dyn MTlsVerifier> = Arc::new(StubMtlsVerifier);
    let d = dispatcher_with(None, Some(mtls), None, &[&tenant]);
    assert!(d.dispatch(tenant.bridge_request()).await.is_ok());
}

#[tokio::test]
async fn dispatch_api_key_happy_path() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let verifier = api_key_verifier_for(AudienceAccount::new(tenant.account.as_str()));
    let d = dispatcher_with(None, None, verifier, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.auth_token = Some("k_live_demo".into());
    });
    assert!(d.dispatch(req).await.is_ok());
}

#[tokio::test]
async fn dispatch_api_key_rejects_audience_mismatch() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let registered = Tenant::generate();
    let verifier = api_key_verifier_for(AudienceAccount::new(registered.account.as_str()));
    let d = dispatcher_with(None, None, verifier, &[&tenant, &registered]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.auth_token = Some("k_live_demo".into());
    });
    assert!(d.dispatch(req).await.is_err());
}

#[tokio::test]
async fn dispatch_rejects_a_request_presenting_only_the_sentinel_jwt() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.pass = None;
        c.nats.connect_opts.auth_token = None;
        c.nats.client_tls = None;
    });
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn dispatch_oidc_verifier_unavailable() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(None, None, None, &[&tenant]);
    let req = tenant.request(&server, |_| {});
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::VerifierUnavailable { scheme: "OIDC" })
    ));
}

#[tokio::test]
async fn dispatch_mtls_verifier_unavailable() {
    let tenant = Tenant::generate();
    let d = dispatcher_with(None, None, None, &[&tenant]);
    let err = d.dispatch(tenant.bridge_request()).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::VerifierUnavailable { scheme: "mTLS" })
    ));
}

#[tokio::test]
async fn dispatch_api_key_verifier_unavailable() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(None, None, None, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.auth_token = Some("k_live_demo".into());
    });
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(CredentialError::VerifierUnavailable { scheme: "API-key" })
    ));
}

#[tokio::test]
async fn dispatch_oidc_via_pass() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let d = dispatcher_with(oidc_stub(), None, None, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.auth_token = None;
        c.nats.connect_opts.pass = Some("eyJhbGciOiJSUzI1NiJ9.e30.c2ln".into());
    });
    assert!(d.dispatch(req).await.is_ok());
}

#[tokio::test]
async fn dispatch_api_key_via_pass() {
    let server = FixtureServer::generate();
    let tenant = Tenant::generate();
    let verifier = api_key_verifier_for(AudienceAccount::new(tenant.account.as_str()));
    let d = dispatcher_with(oidc_stub(), None, verifier, &[&tenant]);
    let req = tenant.request(&server, |c| {
        c.nats.connect_opts.auth_token = None;
        c.nats.connect_opts.pass = Some("k_live_demo".into());
    });
    assert!(d.dispatch(req).await.is_ok());
}
