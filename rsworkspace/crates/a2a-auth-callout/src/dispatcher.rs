use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::account_resolver::AccountResolver;
#[allow(deprecated)]
use crate::credentials::api_key::ApiKeyVerifier;
use crate::credentials::mtls::MTlsVerifier;
use crate::credentials::oidc::{BearerToken, OidcVerifier};
use crate::error::AuthCalloutError;
use crate::jwt::MintedUserJwt;
use crate::signing_key_source::SigningKeySource;
use crate::wire::ServerAuthRequestClaims;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    Oidc,
    MTls,
    ApiKey,
}

#[async_trait::async_trait]
pub trait AuthDispatcher: Send + Sync + 'static {
    async fn dispatch(
        &self,
        request: ServerAuthRequestClaims,
    ) -> Result<MintedUserJwt, AuthCalloutError>;
}

pub struct CalloutDispatcherConfig {
    pub signing_key_source: Arc<dyn SigningKeySource>,
    pub user_jwt_ttl: Duration,
    pub account_resolver: Arc<dyn AccountResolver>,
    pub oidc: Option<Arc<dyn OidcVerifier>>,
    pub mtls: Option<Arc<dyn MTlsVerifier>>,
    #[allow(deprecated)]
    pub api_key: Option<Arc<dyn ApiKeyVerifier>>,
}

pub struct CalloutDispatcher {
    config: CalloutDispatcherConfig,
}

impl CalloutDispatcher {
    pub fn new(config: CalloutDispatcherConfig) -> Self {
        Self { config }
    }

    fn select_scheme(&self, request: &ServerAuthRequestClaims) -> Result<AuthScheme, AuthCalloutError> {
        if request.connect_opts_jwt().is_some() {
            return Ok(AuthScheme::Oidc);
        }
        if request.primary_client_cert().is_some() {
            return Ok(AuthScheme::MTls);
        }
        if request.connect_opts_auth_token().is_some() {
            return Ok(AuthScheme::ApiKey);
        }
        Err(AuthCalloutError::CredentialVerification(
            "no credential material in authorization request".into(),
        ))
    }
}

#[async_trait::async_trait]
impl AuthDispatcher for CalloutDispatcher {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        let requested = request.requested_account()?;
        let account = self.config.account_resolver.resolve(&requested)?;

        let scheme = self.select_scheme(&request)?;
        let claims = match scheme {
            AuthScheme::Oidc => {
                let verifier = self.config.oidc.as_ref().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("OIDC verifier not configured".into())
                })?;
                let token = request.connect_opts_jwt().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("OIDC scheme but connect_opts.jwt missing".into())
                })?;
                verifier
                    .verify(&BearerToken::new(token.to_owned()), &account)
                    .await?
            }
            AuthScheme::MTls => {
                let verifier = self.config.mtls.as_ref().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("mTLS verifier not configured".into())
                })?;
                let pem = request.primary_client_cert().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification(
                        "mTLS scheme but client_tls certs missing".into(),
                    )
                })?;
                verifier.verify(&pem, &account).await?
            }
            AuthScheme::ApiKey => {
                #[allow(deprecated)]
                let verifier = self.config.api_key.as_ref().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("API-key verifier not configured".into())
                })?;
                let key = request.connect_opts_auth_token().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification(
                        "API-key scheme but connect_opts.auth_token missing".into(),
                    )
                })?;
                #[allow(deprecated)]
                let verified = verifier.verify(key).await?;
                if verified.aud != account {
                    return Err(AuthCalloutError::CredentialVerification(
                        "API-key audience does not match resolved account".into(),
                    ));
                }
                verified
            }
        };

        let handle = self.config.signing_key_source.current();
        claims
            .mint(&handle, SystemTime::now(), self.config.user_jwt_ttl)
            .map_err(AuthCalloutError::from)
    }
}

#[cfg(test)]
#[allow(deprecated)]
pub(crate) mod tests {
    use super::*;
    use crate::account_resolver::StaticAccountResolver;
    use crate::credentials::api_key::{ApiKey, ApiKeyEntry, ApiKeyRegistry, HmacApiKeyVerifier};
    use crate::credentials::mtls::ClientCertPem;
    use crate::credentials::oidc::BearerToken;
    use crate::jwt::{
        AudienceAccount, CallerId, ExternalSubject, SpiceDbPrincipal, UserJwtClaims,
    };
    use crate::permissions::IssuedPermissions;
    use crate::signing_key_source::{KeyVersion, StaticSigningKeySource};
    use crate::wire::test_encode::signed_auth_request;
    use crate::wire::{NkeyPublic, ServerAuthRequestEnvelope};
    use nats_jwt_rs::authorization::AuthRequest;
    use nats_jwt_rs::Claims;
    use nkeys::KeyPair;
    use serde_json::json;

    fn encode_fixture_request(
        server: &KeyPair,
        patch: impl FnMut(&mut Claims<AuthRequest>),
    ) -> ServerAuthRequestClaims {
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
        async fn dispatch(
            &self,
            _request: ServerAuthRequestClaims,
        ) -> Result<MintedUserJwt, AuthCalloutError> {
            Err(AuthCalloutError::CredentialVerification("stub: always deny".into()))
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
        async fn verify(
            &self,
            _token: &BearerToken,
            account: &AudienceAccount,
        ) -> Result<UserJwtClaims, AuthCalloutError> {
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
        let resolver: Arc<dyn AccountResolver> = Arc::new(StaticAccountResolver::new(
            allowed.iter().map(|s| s.to_string()),
        ));
        let signing_key_source: Arc<dyn SigningKeySource> = Arc::new(StaticSigningKeySource::new(
            b"dispatcher-test-secret",
            KeyVersion::new("test").expect("test key version"),
        ));
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
        let d = dispatcher_with(
            None,
            None,
            Some(verifier),
            &["tenant-acme", "tenant-foo"],
        );
        let req = encode_fixture_request(&server, |c| {
            c.nats.connect_opts.jwt = None;
            c.nats.connect_opts.auth_token = Some("k_live_demo".into());
        });
        assert!(d.dispatch(req).await.is_err());
    }
}
