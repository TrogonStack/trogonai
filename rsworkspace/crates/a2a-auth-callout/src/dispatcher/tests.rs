use super::*;
use crate::account_resolver::StaticAccountResolver;
use crate::credentials::api_key::{ApiKey, ApiKeyEntry, ApiKeyRegistry, HmacApiKeyVerifier};
use crate::credentials::mtls::ClientCertPem;
use crate::credentials::oidc::BearerToken;
use crate::jwt::{AudienceAccount, CallerId, ExternalSubject, SpiceDbPrincipal, UserJwtClaims};
use crate::permissions::IssuedPermissions;
use crate::signing_key_source::{KeyVersion, StaticSigningKeySource};
use crate::wire::test_encode::signed_auth_request;
use crate::wire::{NkeyPublic, ServerAuthRequestEnvelope};
use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use nkeys::KeyPair;
use serde_json::json;

fn encode_fixture_request(server: &KeyPair, patch: impl FnMut(&mut Claims<AuthRequest>)) -> ServerAuthRequestClaims {
    let user = KeyPair::new_user();
    let token = signed_auth_request(server, &user, patch);
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, None, None)
        .unwrap()
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
    let server = KeyPair::new_account();
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
        })
    }
}

fn dispatcher_with(
    oidc: Option<Arc<dyn OidcVerifier>>,
    mtls: Option<Arc<dyn MTlsVerifier>>,
    api_key: Option<Arc<dyn ApiKeyVerifier>>,
    allowed: &[&str],
) -> CalloutDispatcher {
    let resolver: Arc<dyn AccountResolver> =
        Arc::new(StaticAccountResolver::new(allowed.iter().map(|s| s.to_string())));
    let account = KeyPair::new_account();
    let account_seed = account.seed().expect("account seed");
    let signing_key_source: Arc<dyn SigningKeySource> = Arc::new(
        StaticSigningKeySource::new(&account_seed, KeyVersion::new("test").expect("test key version"))
            .expect("static signing source"),
    );
    CalloutDispatcher::new(CalloutDispatcherConfig {
        signing_key_source,
        user_jwt_ttl: Duration::from_secs(60),
        account_resolver: resolver,
        oidc,
        mtls,
        api_key,
    })
}

#[tokio::test]
async fn dispatch_oidc_happy_path_mints_jwt() {
    let server = KeyPair::new_account();
    let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
    let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |_| {});
    let resp = d.dispatch(req).await.unwrap();
    assert_eq!(resp.as_str().split('.').count(), 3);
}

#[tokio::test]
async fn dispatch_rejects_unknown_account() {
    let server = KeyPair::new_account();
    let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
    let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.user = Some("tenant-evil".into());
        c.nats.client_info.user = "tenant-evil".into();
    });
    assert!(d.dispatch(req).await.is_err());
}

#[tokio::test]
async fn dispatch_rejects_request_without_account() {
    let server = KeyPair::new_account();
    let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
    let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.user = None;
        c.nats.client_info.user = String::new();
    });
    assert!(d.dispatch(req).await.is_err());
}

#[tokio::test]
async fn dispatch_infers_oidc_when_user_jwt_present() {
    let server = KeyPair::new_account();
    let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
    let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |_| {});
    assert!(d.dispatch(req).await.is_ok());
}

#[tokio::test]
async fn dispatch_mtls_when_cert_present() {
    use crate::bridge_mint::{BridgeClientInfo, BridgeMintRequest};

    let mtls: Arc<dyn MTlsVerifier> = Arc::new(StubMtlsVerifier);
    let d = dispatcher_with(None, Some(mtls), None, &["tenant-acme"]);
    let req = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: Some("tenant-acme".into()),
        client_info: Some(BridgeClientInfo {
            client_cert_pem: Some("-----BEGIN CERT-----\n-----END CERT-----".into()),
        }),
        connect_opts: None,
    })
    .unwrap();
    assert!(d.dispatch(req).await.is_ok());
}

#[tokio::test]
async fn dispatch_api_key_happy_path() {
    let server = KeyPair::new_account();
    let mut registry = ApiKeyRegistry::new(b"hmac-test-secret".to_vec());
    let key = ApiKey::new("k_live_demo").unwrap();
    registry.register(
        &key,
        ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("svc/demo"),
            audience: AudienceAccount::new("tenant-acme"),
            external_subject: ExternalSubject::new("svc-demo").unwrap(),
        },
    );
    let verifier: Arc<dyn ApiKeyVerifier> = Arc::new(HmacApiKeyVerifier::new(Arc::new(registry)));
    let d = dispatcher_with(None, None, Some(verifier), &["tenant-acme"]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.jwt = None;
        c.nats.connect_opts.auth_token = Some("k_live_demo".into());
    });
    assert!(d.dispatch(req).await.is_ok());
}

#[tokio::test]
async fn dispatch_api_key_rejects_audience_mismatch() {
    let server = KeyPair::new_account();
    let mut registry = ApiKeyRegistry::new(b"hmac-test-secret".to_vec());
    let key = ApiKey::new("k_live_demo").unwrap();
    registry.register(
        &key,
        ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("svc/demo"),
            audience: AudienceAccount::new("tenant-foo"),
            external_subject: ExternalSubject::new("svc-demo").unwrap(),
        },
    );
    let verifier: Arc<dyn ApiKeyVerifier> = Arc::new(HmacApiKeyVerifier::new(Arc::new(registry)));
    let d = dispatcher_with(None, None, Some(verifier), &["tenant-acme", "tenant-foo"]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.jwt = None;
        c.nats.connect_opts.auth_token = Some("k_live_demo".into());
    });
    assert!(d.dispatch(req).await.is_err());
}

#[tokio::test]
async fn dispatch_rejects_request_without_credential_material() {
    let server = KeyPair::new_account();
    let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
    let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.jwt = None;
        c.nats.connect_opts.pass = None;
        c.nats.connect_opts.auth_token = None;
        c.nats.client_tls = None;
    });
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(crate::error::CredentialError::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn dispatch_oidc_verifier_unavailable() {
    let server = KeyPair::new_account();
    let d = dispatcher_with(None, None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |_| {});
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(crate::error::CredentialError::VerifierUnavailable { scheme: "OIDC" })
    ));
}

#[tokio::test]
async fn dispatch_mtls_verifier_unavailable() {
    use crate::bridge_mint::{BridgeClientInfo, BridgeMintRequest};

    let d = dispatcher_with(None, None, None, &["tenant-acme"]);
    let req = ServerAuthRequestClaims::from_bridge_mint(BridgeMintRequest {
        user_nkey: None,
        user_jwt: None,
        account: Some("tenant-acme".into()),
        client_info: Some(BridgeClientInfo {
            client_cert_pem: Some("-----BEGIN CERT-----\n-----END CERT-----".into()),
        }),
        connect_opts: None,
    })
    .unwrap();
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(crate::error::CredentialError::VerifierUnavailable { scheme: "mTLS" })
    ));
}

#[tokio::test]
async fn dispatch_api_key_verifier_unavailable() {
    let server = KeyPair::new_account();
    let d = dispatcher_with(None, None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.jwt = None;
        c.nats.connect_opts.auth_token = Some("k_live_demo".into());
    });
    let err = d.dispatch(req).await.unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::CredentialVerification(crate::error::CredentialError::VerifierUnavailable {
            scheme: "API-key"
        })
    ));
}

#[tokio::test]
async fn dispatch_oidc_via_opaque_pass() {
    let server = KeyPair::new_account();
    let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
    let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
    let req = encode_fixture_request(&server, |c| {
        c.nats.connect_opts.jwt = None;
        c.nats.connect_opts.pass = Some("opaque-token".into());
    });
    assert!(d.dispatch(req).await.is_ok());
}
