//! Live `nats-server` 2.14.x auth-callout integration tests (task #8).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[allow(deprecated)]
use a2a_auth_callout::credentials::api_key::{
    ApiKey, ApiKeyEntry, ApiKeyRegistry, ApiKeyVerifier, HmacApiKeyVerifier,
};
use a2a_auth_callout::credentials::mtls::{TrustAnchorPem, X509MtlsVerifier, MTlsVerifier};
use a2a_auth_callout::credentials::oidc::{JwksOidcVerifier, OidcIssuerUrl, OidcVerifier};
use a2a_auth_callout::dispatcher::{AuthDispatcher, CalloutDispatcher, CalloutDispatcherConfig};
use a2a_auth_callout::jwt::{decode_nats_user_payload, ExternalSubject, UserJwtClaims};
use a2a_auth_callout::{AudienceAccount, SpiceDbPrincipal};
use a2a_auth_callout::signing_key_source::{
    KeyVersion, SigningKeySource, StaticSigningKeySource,
};
use a2a_auth_callout::{
    AccountResolver, AuthCalloutWireCodec, DenialCategory, MintedUserJwt, NkeyPublic, NkeySeed,
    StaticAccountResolver,
};
use futures::StreamExt;
use async_nats::Auth;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, Jwk, JwkSet, KeyOperations, PublicKeyUse,
    RSAKeyParameters, RSAKeyType,
};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rand::rngs::OsRng;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde::Serialize;
use tempfile::TempDir;
use testcontainers::core::{IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NATS_IMAGE: &str = "nats";
const NATS_TAG: &str = "2.14.1";
const NATS_CLIENT_PORT: u16 = 4222;

const NATS_SERVER_AUTH_CALLOUT_ISSUER: &str =
    "ABJHLOVMPA4CI6R5KLNGOB4GSLNIY7IOUPAJC4YFNDLQVIOBYQGUWVLA";
const CALLOUT_RESPONSE_ISSUER_SEED: &str =
    "SAANDLKMXL6CUS3CP52WIXBEDN6YJ545GDKC65U5JZPPV6WH6ESWUA6YAI";

const TENANT_ACCOUNT: &str = "tenant-acme";
const API_KEY_HMAC: &[u8] = b"integration-api-key-hmac-secret--";
const API_KEY_VALUE: &str = "k_live_integration";
const OIDC_AUDIENCE: &str = "a2a-client";

#[derive(Debug, Clone, PartialEq, Eq)]
struct NatsClientUrl(String);

impl NatsClientUrl {
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MockOidcIssuerUrl(String);

impl MockOidcIssuerUrl {
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
struct MintedJwtCapture(Arc<Mutex<Vec<MintedUserJwt>>>);

impl MintedJwtCapture {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn len(&self) -> usize {
        self.0.lock().expect("mint capture lock").len()
    }

    fn drain(&self) -> Vec<MintedUserJwt> {
        let mut guard = self.0.lock().expect("mint capture lock");
        std::mem::take(&mut *guard)
    }
}

#[derive(Debug, Clone, Default)]
struct DispatchErrorCapture(Arc<Mutex<Vec<String>>>);

impl DispatchErrorCapture {
    fn push(&self, message: impl Into<String>) {
        self.0
            .lock()
            .expect("dispatch error lock")
            .push(message.into());
    }

    fn drain(&self) -> Vec<String> {
        let mut guard = self.0.lock().expect("dispatch error lock");
        std::mem::take(&mut *guard)
    }
}

struct CapturingDispatcher<D> {
    inner: D,
    capture: MintedJwtCapture,
    errors: DispatchErrorCapture,
}

#[async_trait::async_trait]
impl<D: AuthDispatcher> AuthDispatcher for CapturingDispatcher<D> {
    async fn dispatch(
        &self,
        request: a2a_auth_callout::wire::ServerAuthRequestClaims,
    ) -> Result<MintedUserJwt, a2a_auth_callout::AuthCalloutError> {
        match self.inner.dispatch(request).await {
            Ok(minted) => {
                self.capture
                    .0
                    .lock()
                    .expect("mint capture lock")
                    .push(minted.clone());
                Ok(minted)
            }
            Err(err) => {
                self.errors
                    .0
                    .lock()
                    .expect("dispatch error lock")
                    .push(err.to_string());
                Err(err)
            }
        }
    }
}

#[derive(Clone)]
struct NatsClientTls {
    ca_cert: PathBuf,
    client_cert: PathBuf,
    client_key: PathBuf,
}

struct NatsCalloutStack {
    _nats_container: ContainerAsync<GenericImage>,
    _config_dir: Option<TempDir>,
    nats_url: NatsClientUrl,
    capture: MintedJwtCapture,
    dispatch_errors: DispatchErrorCapture,
    signing: Arc<dyn SigningKeySource>,
    _subscriber_task: tokio::task::JoinHandle<Result<(), a2a_auth_callout::AuthCalloutError>>,
}

fn write_config_dir(server_conf: &str) -> TempDir {
    let config_dir = tempfile::tempdir().expect("config tempdir");
    std::fs::write(config_dir.path().join("nats.conf"), server_conf).expect("write nats.conf");
    config_dir
}

async fn connect_callout_service(
    nats_url: &NatsClientUrl,
    client_tls: Option<NatsClientTls>,
) -> async_nats::Client {
    let mut opts = async_nats::ConnectOptions::new().user_and_password("callout".into(), "callout".into());
    if let Some(tls) = client_tls {
        opts = opts
            .add_root_certificates(tls.ca_cert)
            .add_client_certificate(tls.client_cert, tls.client_key);
    }
    opts.connect(nats_url.as_str()).await.unwrap_or_else(|err| {
        panic!("callout service connect to {}: {err:?}", nats_url.as_str());
    })
}

impl NatsCalloutStack {
    async fn start(
        config_root: &Path,
        dispatcher: CalloutDispatcher,
        client_tls: Option<NatsClientTls>,
    ) -> Self {
        let image = GenericImage::new(NATS_IMAGE, NATS_TAG)
            .with_exposed_port(NATS_CLIENT_PORT.tcp())
            .with_wait_for(WaitFor::message_on_either_std("Server is ready"))
            .with_startup_timeout(Duration::from_secs(120))
            .with_cmd(["-c", "/etc/nats/nats.conf"])
            .with_mount(Mount::bind_mount(
                config_root.to_str().expect("utf8 config path"),
                "/etc/nats",
            ));

        let container = image.start().await.expect("start nats-server container");
        let host = container.get_host().await.expect("container host");
        let port = container
            .get_host_port_ipv4(NATS_CLIENT_PORT.tcp())
            .await
            .expect("mapped client port");
        let scheme = if client_tls.is_some() { "tls" } else { "nats" };
        let nats_url = NatsClientUrl(format!("{scheme}://{host}:{port}"));

        let signing: Arc<dyn SigningKeySource> = Arc::new(
            StaticSigningKeySource::new(
                CALLOUT_RESPONSE_ISSUER_SEED,
                KeyVersion::new("integration").expect("key version"),
            )
            .expect("signing source"),
        );

        let server_issuer =
            NkeyPublic::parse(NATS_SERVER_AUTH_CALLOUT_ISSUER).expect("server issuer nkey");
        let callout_seed =
            NkeySeed::parse(CALLOUT_RESPONSE_ISSUER_SEED).expect("callout issuer seed");
        let wire = Arc::new(
            AuthCalloutWireCodec::new(server_issuer, callout_seed, None, None).expect("wire codec"),
        );

        let capture = MintedJwtCapture::new();
        let dispatch_errors = DispatchErrorCapture::default();
        let capturing = CapturingDispatcher {
            inner: dispatcher,
            capture: capture.clone(),
            errors: dispatch_errors.clone(),
        };

        let callout_client = connect_callout_service(&nats_url, client_tls).await;
        let mut auth_sub = callout_client
            .subscribe("$SYS.REQ.USER.AUTH")
            .await
            .expect("subscribe auth callout subject");
        let reply_client = callout_client.clone();
        let wire_task = Arc::clone(&wire);
        let callout_errors = dispatch_errors.clone();
        let subscriber_task = tokio::spawn(async move {
            while let Some(msg) = auth_sub.next().await {
                let Some(reply) = msg.reply else {
                    callout_errors.push("auth callout request without reply subject");
                    continue;
                };
                let request = match wire_task.decode_request(msg.payload.to_vec(), msg.headers.as_ref())
                {
                    Ok(request) => request,
                    Err(err) => {
                        callout_errors.push(format!("decode auth request: {err}"));
                        continue;
                    }
                };
                match capturing.dispatch(request.clone()).await {
                    Ok(minted) => {
                        if let Ok(payload) = wire_task.encode_success(&request, minted) {
                            let _ = reply_client
                                .publish(reply.to_string(), payload.into())
                                .await;
                        }
                    }
                    Err(err) => {
                        let category = DenialCategory::from_auth_callout_error(&err);
                        if let Ok(payload) =
                            wire_task.encode_denial(&request, category.as_str().to_owned())
                        {
                            let _ = reply_client
                                .publish(reply.to_string(), payload.into())
                                .await;
                        }
                    }
                }
            }
            Ok(())
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !subscriber_task.is_finished(),
            "auth callout subscriber exited before handling requests"
        );

        Self {
            _nats_container: container,
            _config_dir: None,
            nats_url,
            capture,
            dispatch_errors,
            signing,
            _subscriber_task: subscriber_task,
        }
    }

    async fn start_owned(
        config_dir: TempDir,
        dispatcher: CalloutDispatcher,
        client_tls: Option<NatsClientTls>,
    ) -> Self {
        let mut stack = Self::start(config_dir.path(), dispatcher, client_tls).await;
        stack._config_dir = Some(config_dir);
        stack
    }
}

fn base_server_config(tls_block: &str) -> String {
    format!(
        r#"
port: {NATS_CLIENT_PORT}
server_name: a2a-callout-it

accounts {{
  AUTH {{
    users: [ {{ user: callout, password: callout }} ]
  }}
  APP {{}}
  SYS {{}}
}}
system_account: SYS

authorization {{
  timeout: 2s
  auth_callout {{
    issuer: {NATS_SERVER_AUTH_CALLOUT_ISSUER}
    account: AUTH
    auth_users: [ callout ]
  }}
}}
{tls_block}
"#
    )
}

fn b64url_uint_be(bytes: &[u8]) -> String {
    let start = bytes
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(bytes.len().saturating_sub(1));
    let trimmed = if start >= bytes.len() {
        &bytes[bytes.len().saturating_sub(1)..]
    } else {
        &bytes[start..]
    };
    URL_SAFE_NO_PAD.encode(trimmed)
}

fn test_jwks_and_encoding_key(rng: &mut OsRng) -> (JwkSet, EncodingKey) {
    let key = RsaPrivateKey::new(rng, 2048).expect("rsa key");
    let encoding_key = EncodingKey::from_rsa_pem(
        key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .expect("pem")
            .as_bytes(),
    )
    .expect("encoding key");
    let public = key.to_public_key();
    let n = b64url_uint_be(&public.n().to_bytes_be());
    let e = b64url_uint_be(&public.e().to_bytes_be());
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_operations: Some(vec![KeyOperations::Sign]),
            key_id: Some("test-kid".into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
            key_type: RSAKeyType::RSA,
            n,
            e,
        }),
    };
    (JwkSet { keys: vec![jwk] }, encoding_key)
}

async fn mock_oidc_issuer() -> (MockServer, MockOidcIssuerUrl, JwkSet, EncodingKey) {
    let server = MockServer::start().await;
    let issuer = MockOidcIssuerUrl(server.uri());
    let (jwks, enc) = test_jwks_and_encoding_key(&mut OsRng);
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            format!(r#"{{"jwks_uri":"{}/jwks"}}"#, issuer.as_str()),
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
        .mount(&server)
        .await;
    (server, issuer, jwks, enc)
}

fn mint_oidc_bearer(issuer: &MockOidcIssuerUrl, enc: &EncodingKey, sub: &str) -> String {
    #[derive(Serialize)]
    struct IdClaims {
        sub: String,
        iss: String,
        aud: String,
        exp: u64,
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let id = IdClaims {
        sub: sub.into(),
        iss: issuer.as_str().to_owned(),
        aud: OIDC_AUDIENCE.into(),
        exp: now + 600,
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-kid".into());
    encode(&header, &id, enc).expect("oidc bearer encode")
}

fn decode_captured(
    minted: &MintedUserJwt,
    signing: &dyn SigningKeySource,
) -> UserJwtClaims {
    UserJwtClaims::verify_with_source(minted.as_str(), signing).expect("decode minted user jwt")
}

fn assert_minted_nats_user_jwt_shape(token: &str) {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header: serde_json::Value =
        serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[0]).unwrap())
            .unwrap();
    assert_eq!(header["alg"], "ed25519-nkey");
    let payload = decode_nats_user_payload(token).expect("payload");
    assert_eq!(payload["nats"]["type"], "user");
    assert_eq!(payload["aud"], TENANT_ACCOUNT);
    assert!(!payload["caller_id"].as_str().unwrap_or("").is_empty());
}

fn assert_tenant_acl(claims: &UserJwtClaims) {
    assert_eq!(claims.aud.as_str(), TENANT_ACCOUNT);
    assert!(!claims.caller_id.as_str().is_empty());
    assert_eq!(claims.nats_permissions.publish_allow.len(), 1);
    assert_eq!(
        claims.nats_permissions.publish_allow[0].as_str(),
        "a2a.gateway.>"
    );
    let subs: Vec<&str> = claims
        .nats_permissions
        .subscribe_allow
        .iter()
        .map(|p| p.as_str())
        .collect();
    assert_eq!(subs.len(), 2);
    let inbox = format!("_INBOX.{}.>", claims.caller_id.as_str());
    let push = format!("a2a.push.{}.>", claims.caller_id.as_str());
    assert!(subs.contains(&inbox.as_str()));
    assert!(subs.contains(&push.as_str()));
}

fn assert_stable_caller_id(
    minted: &[MintedUserJwt],
    signing: &dyn SigningKeySource,
    dispatch_errors: &[String],
) {
    assert!(
        minted.len() >= 2,
        "expected at least two callout mints, got {}; decode/dispatch errors: {dispatch_errors:?}",
        minted.len()
    );
    let first = decode_captured(&minted[0], signing);
    let second = decode_captured(&minted[1], signing);
    assert_tenant_acl(&first);
    assert_tenant_acl(&second);
    assert_eq!(first.caller_id, second.caller_id);
    assert_minted_nats_user_jwt_shape(minted[0].as_str());
}

async fn assert_connect_admits<F>(
    nats_url: &NatsClientUrl,
    client_tls: Option<NatsClientTls>,
    build_auth: F,
) where
    F: Fn() -> Auth + Clone + Send + Sync + 'static,
{
    let client = connect_with_auth_callback(nats_url, client_tls, build_auth)
        .await
        .expect("NATS connect should succeed after callout minted NATS User JWT");
    client.flush().await.expect("connected client flush");
}

async fn connect_with_auth_callback<F>(
    nats_url: &NatsClientUrl,
    client_tls: Option<NatsClientTls>,
    build_auth: F,
) -> Result<async_nats::Client, async_nats::ConnectError>
where
    F: Fn() -> Auth + Clone + Send + Sync + 'static,
{
    let mut opts = async_nats::ConnectOptions::with_auth_callback(move |_| {
        let build = build_auth.clone();
        async move { Ok(build()) }
    });
    if let Some(tls) = client_tls {
        opts = opts
            .add_root_certificates(tls.ca_cert)
            .add_client_certificate(tls.client_cert, tls.client_key);
    }
    opts.connect(nats_url.as_str()).await
}

async fn drive_callout_attempts<F>(
    nats_url: &NatsClientUrl,
    client_tls: Option<NatsClientTls>,
    capture: &MintedJwtCapture,
    build_auth: F,
) where
    F: Fn() -> Auth + Clone + Send + Sync + 'static,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline && capture.len() < 2 {
        let _ = connect_with_auth_callback(nats_url, client_tls.clone(), build_auth.clone())
            .await
            .ok();
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn dispatcher_with_oidc(issuer: OidcIssuerUrl, jwks: JwkSet) -> CalloutDispatcher {
    let signing: Arc<dyn SigningKeySource> = Arc::new(
        StaticSigningKeySource::new(
            CALLOUT_RESPONSE_ISSUER_SEED,
            KeyVersion::new("integration").expect("key version"),
        )
        .expect("signing source"),
    );
    let resolver: Arc<dyn AccountResolver> =
        Arc::new(StaticAccountResolver::new([TENANT_ACCOUNT.to_owned()]));
    let oidc: Arc<dyn OidcVerifier> = Arc::new(JwksOidcVerifier::with_static_jwks(
        issuer,
        vec![OIDC_AUDIENCE.into()],
        jwks,
    ));
    CalloutDispatcher::new(CalloutDispatcherConfig {
        signing_key_source: signing,
        user_jwt_ttl: Duration::from_secs(300),
        account_resolver: resolver,
        oidc: Some(oidc),
        mtls: None,
        api_key: None,
    })
}

fn dispatcher_with_mtls(trust_anchor_pem: String) -> CalloutDispatcher {
    let signing: Arc<dyn SigningKeySource> = Arc::new(
        StaticSigningKeySource::new(
            CALLOUT_RESPONSE_ISSUER_SEED,
            KeyVersion::new("integration").expect("key version"),
        )
        .expect("signing source"),
    );
    let resolver: Arc<dyn AccountResolver> =
        Arc::new(StaticAccountResolver::new([TENANT_ACCOUNT.to_owned()]));
    let mtls: Arc<dyn MTlsVerifier> = Arc::new(X509MtlsVerifier::new(TrustAnchorPem::new(
        trust_anchor_pem,
    )));
    CalloutDispatcher::new(CalloutDispatcherConfig {
        signing_key_source: signing,
        user_jwt_ttl: Duration::from_secs(300),
        account_resolver: resolver,
        oidc: None,
        mtls: Some(mtls),
        api_key: None,
    })
}

#[allow(deprecated)]
fn dispatcher_with_api_key() -> CalloutDispatcher {
    let signing: Arc<dyn SigningKeySource> = Arc::new(
        StaticSigningKeySource::new(
            CALLOUT_RESPONSE_ISSUER_SEED,
            KeyVersion::new("integration").expect("key version"),
        )
        .expect("signing source"),
    );
    let resolver: Arc<dyn AccountResolver> =
        Arc::new(StaticAccountResolver::new([TENANT_ACCOUNT.to_owned()]));
    let mut registry = ApiKeyRegistry::new(API_KEY_HMAC.to_vec());
    let key = ApiKey::new(API_KEY_VALUE).expect("api key");
    registry.register(
        &key,
        ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("svc/integration"),
            audience: AudienceAccount::new(TENANT_ACCOUNT),
            external_subject: ExternalSubject::new("svc-integration").expect("external subject"),
        },
    );
    let api_key: Arc<dyn ApiKeyVerifier> = Arc::new(HmacApiKeyVerifier::new(Arc::new(registry)));
    CalloutDispatcher::new(CalloutDispatcherConfig {
        signing_key_source: signing,
        user_jwt_ttl: Duration::from_secs(300),
        account_resolver: resolver,
        oidc: None,
        mtls: None,
        api_key: Some(api_key),
    })
}

struct TlsFixture {
    dir: TempDir,
    trust_anchor_pem: String,
    client_cert: PathBuf,
    client_key: PathBuf,
    ca_cert: PathBuf,
}

fn write_tls_fixture() -> TlsFixture {
    use std::net::{IpAddr, Ipv4Addr};

    use rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, SanType,
    };

    let dir = tempfile::tempdir().expect("tls tempdir");
    let cert_dir = dir.path().join("certs");
    std::fs::create_dir_all(&cert_dir).expect("certs dir");

    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "integration-ca");
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).expect("ca cert");
    let ca_pem = ca.pem();

    let server_key = KeyPair::generate().expect("server key");
    let mut server_dn = DistinguishedName::new();
    server_dn.push(DnType::CommonName, "nats-integration");
    let mut server_params = CertificateParams::default();
    server_params.distinguished_name = server_dn;
    server_params.subject_alt_names = vec![
        SanType::DnsName("localhost".try_into().expect("dns san")),
        SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
    ];
    let server_cert = server_params
        .signed_by(&server_key, &ca, &ca_key)
        .expect("server cert");

    let ee_key = KeyPair::generate().expect("ee key");
    let mut ee_dn = DistinguishedName::new();
    ee_dn.push(DnType::CommonName, "integration-client");
    let mut ee_params = CertificateParams::default();
    ee_params.distinguished_name = ee_dn;
    let ee = ee_params.signed_by(&ee_key, &ca, &ca_key).expect("ee cert");

    std::fs::write(cert_dir.join("server.pem"), server_cert.pem()).expect("server pem");
    std::fs::write(
        cert_dir.join("server-key.pem"),
        server_key.serialize_pem(),
    )
    .expect("server key");
    std::fs::write(cert_dir.join("ca.pem"), &ca_pem).expect("ca pem");
    std::fs::write(cert_dir.join("client.pem"), ee.pem()).expect("client pem");
    std::fs::write(cert_dir.join("client-key.pem"), ee_key.serialize_pem()).expect("client key");

    std::fs::write(dir.path().join("nats.conf"), mtls_server_config()).expect("nats.conf");

    TlsFixture {
        ca_cert: cert_dir.join("ca.pem"),
        dir,
        trust_anchor_pem: ca_pem,
        client_cert: cert_dir.join("client.pem"),
        client_key: cert_dir.join("client-key.pem"),
    }
}

fn mtls_server_config() -> String {
    let tls = r#"
tls {
  cert_file: "/etc/nats/certs/server.pem"
  key_file: "/etc/nats/certs/server-key.pem"
  ca_file: "/etc/nats/certs/ca.pem"
  verify: true
}
"#;
    base_server_config(tls)
}

#[tokio::test]
#[ignore = "requires Docker (task #8)"]
async fn oidc_bearer_callout_against_nats_server_mints_caller_acl() {
    let (_oidc_mock, issuer_url, jwks, enc) = mock_oidc_issuer().await;
    let issuer = OidcIssuerUrl::parse(issuer_url.as_str()).expect("issuer url");
    let dispatcher = dispatcher_with_oidc(issuer, jwks);
    let config_dir = write_config_dir(&base_server_config(""));
    let stack = NatsCalloutStack::start_owned(config_dir, dispatcher, None).await;

    let bearer = mint_oidc_bearer(&issuer_url, &enc, "user-integration-oidc");
    drive_callout_attempts(&stack.nats_url, None, &stack.capture, {
        let bearer = bearer.clone();
        move || {
            let mut auth = Auth::new();
            auth.jwt = Some(bearer.clone());
            auth.password = Some(bearer.clone());
            auth.username = Some(TENANT_ACCOUNT.into());
            auth
        }
    })
    .await;

    let errors = stack.dispatch_errors.drain();
    assert_stable_caller_id(&stack.capture.drain(), stack.signing.as_ref(), &errors);
    assert_connect_admits(&stack.nats_url, None, {
        let bearer = bearer.clone();
        move || {
            let mut auth = Auth::new();
            auth.jwt = Some(bearer.clone());
            auth.password = Some(bearer.clone());
            auth.username = Some(TENANT_ACCOUNT.into());
            auth
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "requires Docker (task #8)"]
async fn mtls_callout_against_nats_server_mints_caller_acl() {
    let tls = write_tls_fixture();
    let dispatcher = dispatcher_with_mtls(tls.trust_anchor_pem.clone());
    let client_tls = NatsClientTls {
        ca_cert: tls.ca_cert.clone(),
        client_cert: tls.client_cert.clone(),
        client_key: tls.client_key.clone(),
    };
    let stack = NatsCalloutStack::start_owned(tls.dir, dispatcher, Some(client_tls.clone())).await;

    drive_callout_attempts(&stack.nats_url, Some(client_tls.clone()), &stack.capture, {
        move || {
            let mut auth = Auth::new();
            auth.username = Some(TENANT_ACCOUNT.into());
            auth
        }
    })
    .await;

    let errors = stack.dispatch_errors.drain();
    assert_stable_caller_id(&stack.capture.drain(), stack.signing.as_ref(), &errors);
    assert_connect_admits(&stack.nats_url, Some(client_tls), {
        move || {
            let mut auth = Auth::new();
            auth.username = Some(TENANT_ACCOUNT.into());
            auth
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "requires Docker (task #8)"]
async fn api_key_callout_against_nats_server_mints_caller_acl() {
    let dispatcher = dispatcher_with_api_key();
    let config_dir = write_config_dir(&base_server_config(""));
    let stack = NatsCalloutStack::start_owned(config_dir, dispatcher, None).await;

    drive_callout_attempts(&stack.nats_url, None, &stack.capture, {
        let key = API_KEY_VALUE.to_owned();
        move || {
            let mut auth = Auth::new();
            auth.token = Some(key.clone());
            auth.username = Some(TENANT_ACCOUNT.into());
            auth
        }
    })
    .await;

    let errors = stack.dispatch_errors.drain();
    assert_stable_caller_id(&stack.capture.drain(), stack.signing.as_ref(), &errors);
    assert_connect_admits(&stack.nats_url, None, {
        let key = API_KEY_VALUE.to_owned();
        move || {
            let mut auth = Auth::new();
            auth.token = Some(key.clone());
            auth.username = Some(TENANT_ACCOUNT.into());
            auth
        }
    })
    .await;
}
