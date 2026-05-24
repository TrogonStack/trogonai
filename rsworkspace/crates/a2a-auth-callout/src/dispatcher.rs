// TODO(spec): The exact wire encoding of $SYS.REQ.USER.AUTH request/reply payloads
// is defined by the NATS server version and auth callout extension.
// Reference: https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout
//
// The fields below are derived from the illustrative shape in
// docs/A2A_AUTH_CALLOUT_SKETCH.md §2. Replace with the actual nkeys/jwt-encoded
// request struct once the NATS auth callout wire format is pinned for this deployment.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::account_resolver::{AccountResolver, RequestedAccount};
#[allow(deprecated)]
use crate::credentials::api_key::ApiKeyVerifier;
use crate::credentials::mtls::{ClientCertPem, MTlsVerifier};
use crate::credentials::oidc::{BearerToken, OidcVerifier};
use crate::error::AuthCalloutError;
use crate::signing_key_source::SigningKeySource;

/// Illustrative shape of the auth callout request published by the NATS server
/// on `$SYS.REQ.USER.AUTH`.
///
/// Field names follow the sketch in `docs/A2A_AUTH_CALLOUT_SKETCH.md` §2.
/// The real server payload may be NKey-signed and/or JWT-encoded; replace this
/// struct once the wire format is confirmed for the target NATS operator setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCalloutRequest {
    /// Client-supplied credential material (NKey public key or JWT bearer).
    pub user_nkey: Option<String>,
    /// Client-supplied JWT if the connect options included an OIDC bearer token.
    pub user_jwt: Option<String>,
    /// Target Account the client is attempting to join (caller-claimed; verified by resolver).
    pub account: Option<String>,
    /// Opaque client connection metadata (TLS state, IP, client library id).
    pub client_info: Option<ClientInfo>,
    /// Optional tags or headers from the client connect options.
    pub connect_opts: Option<ConnectOpts>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientInfo {
    /// PEM-encoded client certificate chain when the connection used mTLS.
    pub client_cert_pem: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectOpts {
    /// Caller-selected auth scheme hint; the dispatcher cross-checks against
    /// available credential material in the request.
    pub auth_scheme: Option<AuthScheme>,
    /// Transitional API-key bearer; used only when `auth_scheme == ApiKey`.
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    Oidc,
    MTls,
    ApiKey,
}

/// Illustrative shape of a successful auth callout reply (callout → server).
///
/// On success the service must return a signed User JWT bound to the tenant Account.
/// On failure it must return an authorization denied indicator — represented here as
/// an `Err` from the `AuthDispatcher`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCalloutResponse {
    /// Signed User JWT bound to the tenant Account (short TTL).
    pub user_jwt: String,
}

/// Trait that the subscriber loop delegates auth decisions to.
///
/// Inject a test double via `Subscriber::new` to unit-test the loop without a
/// live NATS connection.
#[async_trait::async_trait]
pub trait AuthDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, request: AuthCalloutRequest) -> Result<AuthCalloutResponse, AuthCalloutError>;
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

/// Production dispatcher wiring the verifier library + account resolver + JWT mint.
///
/// Construction is split from `dispatch` so the main binary can build it from
/// env-driven config while tests can supply trait-mocked verifiers directly.
pub struct CalloutDispatcher {
    config: CalloutDispatcherConfig,
}

impl CalloutDispatcher {
    pub fn new(config: CalloutDispatcherConfig) -> Self {
        Self { config }
    }

    fn select_scheme(&self, request: &AuthCalloutRequest) -> Result<AuthScheme, AuthCalloutError> {
        if let Some(opts) = &request.connect_opts
            && let Some(scheme) = opts.auth_scheme
        {
            return Ok(scheme);
        }
        if request.user_jwt.is_some() {
            return Ok(AuthScheme::Oidc);
        }
        if request
            .client_info
            .as_ref()
            .and_then(|c| c.client_cert_pem.as_ref())
            .is_some()
        {
            return Ok(AuthScheme::MTls);
        }
        if request
            .connect_opts
            .as_ref()
            .and_then(|c| c.api_key.as_ref())
            .is_some()
        {
            return Ok(AuthScheme::ApiKey);
        }
        Err(AuthCalloutError::CredentialVerification(
            "no credential material in request".into(),
        ))
    }
}

#[async_trait::async_trait]
impl AuthDispatcher for CalloutDispatcher {
    async fn dispatch(&self, request: AuthCalloutRequest) -> Result<AuthCalloutResponse, AuthCalloutError> {
        let requested = request
            .account
            .as_ref()
            .ok_or_else(|| AuthCalloutError::CredentialVerification("request missing account".into()))?;
        let requested = RequestedAccount::new(requested.clone())
            .map_err(AuthCalloutError::from)?;
        let account = self.config.account_resolver.resolve(&requested)?;

        let scheme = self.select_scheme(&request)?;
        let claims = match scheme {
            AuthScheme::Oidc => {
                let verifier = self.config.oidc.as_ref().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("OIDC verifier not configured".into())
                })?;
                let token = request.user_jwt.clone().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("OIDC scheme but user_jwt missing".into())
                })?;
                verifier.verify(&BearerToken::new(token), &account).await?
            }
            AuthScheme::MTls => {
                let verifier = self.config.mtls.as_ref().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("mTLS verifier not configured".into())
                })?;
                let pem = request
                    .client_info
                    .as_ref()
                    .and_then(|c| c.client_cert_pem.clone())
                    .ok_or_else(|| {
                        AuthCalloutError::CredentialVerification(
                            "mTLS scheme but client_cert_pem missing".into(),
                        )
                    })?;
                verifier.verify(&ClientCertPem::new(pem), &account).await?
            }
            AuthScheme::ApiKey => {
                #[allow(deprecated)]
                let verifier = self.config.api_key.as_ref().ok_or_else(|| {
                    AuthCalloutError::CredentialVerification("API-key verifier not configured".into())
                })?;
                let key = request
                    .connect_opts
                    .as_ref()
                    .and_then(|c| c.api_key.clone())
                    .ok_or_else(|| {
                        AuthCalloutError::CredentialVerification(
                            "API-key scheme but connect_opts.api_key missing".into(),
                        )
                    })?;
                #[allow(deprecated)]
                let verified = verifier.verify(&key).await?;
                if verified.aud != account {
                    return Err(AuthCalloutError::CredentialVerification(
                        "API-key audience does not match resolved account".into(),
                    ));
                }
                verified
            }
        };

        let handle = self.config.signing_key_source.current();
        let mut claims = claims;
        claims.kid = handle.version().clone();
        let token = claims
            .mint(&handle, SystemTime::now(), self.config.user_jwt_ttl)
            .map_err(AuthCalloutError::from)?;
        Ok(AuthCalloutResponse { user_jwt: token })
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
    use crate::signing_key_source::{KeyVersion, StaticSigningKeySource};
    use crate::permissions::IssuedPermissions;
    use serde_json::json;

    pub struct AlwaysDenyDispatcher;

    #[async_trait::async_trait]
    impl AuthDispatcher for AlwaysDenyDispatcher {
        async fn dispatch(&self, _request: AuthCalloutRequest) -> Result<AuthCalloutResponse, AuthCalloutError> {
            Err(AuthCalloutError::CredentialVerification("stub: always deny".into()))
        }
    }

    #[tokio::test]
    async fn always_deny_returns_err() {
        let d = AlwaysDenyDispatcher;
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: None,
            account: None,
            client_info: None,
            connect_opts: None,
        };
        assert!(d.dispatch(req).await.is_err());
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
        #[allow(deprecated)] api_key: Option<Arc<dyn ApiKeyVerifier>>,
        allowed: &[&str],
    ) -> CalloutDispatcher {
        let resolver: Arc<dyn AccountResolver> = Arc::new(StaticAccountResolver::new(
            allowed.iter().map(|s| s.to_string()),
        ));
        let signing_key_source: Arc<dyn SigningKeySource> = Arc::new(StaticSigningKeySource::new(
            b"dispatcher-test-secret",
            KeyVersion::new("test").expect("fixture version"),
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
        let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
        let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: Some("opaque.jwt.token".into()),
            account: Some("tenant-acme".into()),
            client_info: None,
            connect_opts: Some(ConnectOpts {
                auth_scheme: Some(AuthScheme::Oidc),
                api_key: None,
            }),
        };
        let resp = d.dispatch(req).await.unwrap();
        assert_eq!(resp.user_jwt.split('.').count(), 3);
    }

    #[tokio::test]
    async fn dispatch_rejects_unknown_account() {
        let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
        let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: Some("t".into()),
            account: Some("tenant-evil".into()),
            client_info: None,
            connect_opts: Some(ConnectOpts {
                auth_scheme: Some(AuthScheme::Oidc),
                api_key: None,
            }),
        };
        assert!(d.dispatch(req).await.is_err());
    }

    #[tokio::test]
    async fn dispatch_rejects_request_without_account() {
        let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
        let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: Some("t".into()),
            account: None,
            client_info: None,
            connect_opts: None,
        };
        assert!(d.dispatch(req).await.is_err());
    }

    #[tokio::test]
    async fn dispatch_infers_oidc_when_user_jwt_present() {
        let oidc: Arc<dyn OidcVerifier> = Arc::new(StubOidcVerifier { sub: "user-1" });
        let d = dispatcher_with(Some(oidc), None, None, &["tenant-acme"]);
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: Some("t".into()),
            account: Some("tenant-acme".into()),
            client_info: None,
            connect_opts: None,
        };
        assert!(d.dispatch(req).await.is_ok());
    }

    #[tokio::test]
    async fn dispatch_mtls_when_cert_present_and_scheme_unset() {
        let mtls: Arc<dyn MTlsVerifier> = Arc::new(StubMtlsVerifier);
        let d = dispatcher_with(None, Some(mtls), None, &["tenant-acme"]);
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: None,
            account: Some("tenant-acme".into()),
            client_info: Some(ClientInfo {
                client_cert_pem: Some("-----BEGIN CERT-----\n-----END CERT-----".into()),
            }),
            connect_opts: None,
        };
        assert!(d.dispatch(req).await.is_ok());
    }

    #[tokio::test]
    async fn dispatch_rejects_when_no_credential() {
        let d = dispatcher_with(None, None, None, &["tenant-acme"]);
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: None,
            account: Some("tenant-acme".into()),
            client_info: None,
            connect_opts: None,
        };
        assert!(d.dispatch(req).await.is_err());
    }

    #[tokio::test]
    async fn dispatch_api_key_happy_path() {
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
        #[allow(deprecated)]
        let verifier: Arc<dyn ApiKeyVerifier> = Arc::new(HmacApiKeyVerifier::new(Arc::new(registry)));
        let d = dispatcher_with(None, None, Some(verifier), &["tenant-acme"]);
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: None,
            account: Some("tenant-acme".into()),
            client_info: None,
            connect_opts: Some(ConnectOpts {
                auth_scheme: Some(AuthScheme::ApiKey),
                api_key: Some("k_live_demo".into()),
            }),
        };
        assert!(d.dispatch(req).await.is_ok());
    }

    #[tokio::test]
    async fn dispatch_api_key_rejects_audience_mismatch() {
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
        #[allow(deprecated)]
        let verifier: Arc<dyn ApiKeyVerifier> = Arc::new(HmacApiKeyVerifier::new(Arc::new(registry)));
        let d = dispatcher_with(
            None,
            None,
            Some(verifier),
            &["tenant-acme", "tenant-foo"],
        );
        let req = AuthCalloutRequest {
            user_nkey: None,
            user_jwt: None,
            account: Some("tenant-acme".into()),
            client_info: None,
            connect_opts: Some(ConnectOpts {
                auth_scheme: Some(AuthScheme::ApiKey),
                api_key: Some("k_live_demo".into()),
            }),
        };
        assert!(d.dispatch(req).await.is_err());
    }
}
