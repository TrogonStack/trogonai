//! Live `nats-server` 2.14.x auth-callout integration tests against an
//! operator-mode server shaped like `devops/nats/nats-server.conf`: TLS with
//! client verification, a full resolver preloaded with SYS, AUTH, PLATFORM and
//! one tenant account, and an `AUTH` sentinel as the default identity.
//!
//! These tests intentionally use `async_nats` directly instead of the
//! `trogon-nats` abstraction layer because `trogon-nats` currently only
//! exposes static credential auth (nkey / user-password / token) and the
//! callout flow needs `ConnectOptions::with_auth_callback` to drive the
//! server-side challenge/response, a capability `trogon-nats` does not
//! re-export today. Extending `trogon-nats` to expose callback-based auth
//! would let this harness move back behind the abstraction.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[allow(deprecated)]
use a2a_auth_callout::credentials::api_key::{ApiKey, ApiKeyEntry, ApiKeyRegistry, ApiKeyVerifier, HmacApiKeyVerifier};
use a2a_auth_callout::credentials::mtls::{MTlsVerifier, TrustAnchorPem, X509MtlsVerifier};
use a2a_auth_callout::credentials::oidc::{JwksOidcVerifier, OidcIssuerUrl, OidcVerifier};
use a2a_auth_callout::dispatcher::{AuthDispatcher, CalloutDispatcher, CalloutDispatcherConfig};
use a2a_auth_callout::jwt::{ExternalSubject, UserJwtClaims, decode_nats_user_payload};
use a2a_auth_callout::signing_key_source::{KeyVersion, SigningKeySource, StaticSigningKeySource};
use a2a_auth_callout::tenant_admission::{TenantMembership, TenantMemberships};
use a2a_auth_callout::wire::ServerAuthRequestClaims;
use a2a_auth_callout::{
    AccountSigningKey, AuthAccount, AuthCalloutWireCodec, CalloutXkey, DenialCategory, MintedUserJwt, TenantAccount,
    TenantSigningKeys,
};
use a2a_auth_callout::{AudienceAccount, SpiceDbPrincipal};
use a2a_auth_callout::{BridgeClientInfo, BridgeMintRequest, SubjectPattern, subject_principal_from_minted_jwt};
use a2a_nats::nats::subjects::agents::MessageSendSubject;
use a2a_nats::nats::subjects::gateway::{GatewayEgressEventsSubject, GatewayIngressSubject};
use a2a_nats::nats::subjects::subscriptions::CallerTaskEventsSubject;
use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use a2a_nats::{A2aAgentId, A2aMethod, A2aPrefix, A2aTaskId};
use async_nats::{Auth, AuthError, ConnectOptions, Event, ServerError};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::StreamExt;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, Jwk, JwkSet, KeyOperations, PublicKeyUse, RSAKeyParameters, RSAKeyType,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use nats_jwt_rs::account::{Account, ExternalAuthorization, JetStreamLimits, OperatorLimits};
use nats_jwt_rs::operator::Operator;
use nats_jwt_rs::types::{NO_LIMIT, Permission, Permissions, SigningKey};
use nats_jwt_rs::user::{User, UserPermissionLimits};
use rand_core::OsRng;
use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair, SanType};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use serde::Serialize;
use tempfile::TempDir;
use testcontainers::core::{IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use trogon_identity_types::{PrincipalId, PrincipalKind, SubjectPrincipal};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NATS_IMAGE: &str = "nats";
const NATS_TAG: &str = "2.14.2-alpine";
const NATS_CLIENT_PORT: u16 = 4222;
const NATS_CONFIG_ROOT: &str = "/etc/nats";
const AUTH_CALLOUT_SUBJECT: &str = "$SYS.REQ.USER.AUTH";
const A2A_PREFIX: &str = "a2a";
const TENANT_OBSERVED_SUBJECTS: &str = "a2a.v1.>";
const INVALIDATION_STREAM: &str = "SECRETS_INVALIDATION";
const INVALIDATION_SUBJECTS: &str = "secrets.invalidation.v1.*.>";
const INVALIDATION_WATCH: &str = "secrets.invalidation.v1.>";
/// JetStream's error code for a stream that does not exist.
const JS_STREAM_NOT_FOUND: i64 = 10059;

const API_KEY_HMAC: &[u8] = b"integration-api-key-hmac-secret--";
const API_KEY_VALUE: &str = "k_live_integration";
const OIDC_AUDIENCE: &str = "a2a-client";

struct TenantFixture {
    account: TenantAccount,
    user_issuer: nkeys::KeyPair,
    callout_signing: nkeys::KeyPair,
}

impl TenantFixture {
    fn generate() -> Self {
        Self {
            account: TenantAccount::parse(&nkeys::KeyPair::new_account().public_key()).expect("tenant account"),
            user_issuer: nkeys::KeyPair::new_account(),
            callout_signing: nkeys::KeyPair::new_account(),
        }
    }

    fn name(&self) -> String {
        self.account.as_str().to_owned()
    }

    fn callout_signing_seed(&self) -> String {
        self.callout_signing.seed().expect("tenant callout signing seed")
    }

    fn signing_keys(&self) -> TenantSigningKeys {
        let signing_key = AccountSigningKey::from_seed(&self.callout_signing_seed()).expect("tenant signing key");
        TenantSigningKeys::new([(self.account.clone(), signing_key)]).expect("tenant signing keys")
    }

    fn verification_source(&self) -> Arc<dyn SigningKeySource> {
        Arc::new(
            StaticSigningKeySource::new(
                &self.callout_signing_seed(),
                KeyVersion::new("current").expect("key version"),
            )
            .expect("tenant verification source"),
        )
    }
}

#[derive(Clone)]
struct NatsUser {
    jwt: String,
    key: Arc<nkeys::KeyPair>,
}

impl NatsUser {
    fn issue(name: &str, account: &str, issuer: &nkeys::KeyPair, permissions: UserPermissionLimits) -> Self {
        let key = nkeys::KeyPair::new_user();
        let mut claims = User::new_claims(name.to_owned(), key.public_key());
        claims.nats.issuer_account = Some(account.to_owned());
        claims.nats.permissions = permissions;
        Self {
            jwt: claims.encode(issuer).expect("encode user jwt"),
            key: Arc::new(key),
        }
    }

    fn public_key(&self) -> String {
        self.key.public_key()
    }

    fn connect_options(&self) -> ConnectOptions {
        let user = self.clone();
        ConnectOptions::with_auth_callback(move |nonce| {
            let user = user.clone();
            async move {
                let mut auth = Auth::new();
                auth.signature = Some(user.key.sign(&nonce).map_err(AuthError::new)?);
                auth.jwt = Some(user.jwt);
                Ok(auth)
            }
        })
    }
}

fn sentinel_permissions() -> UserPermissionLimits {
    let deny_all = Permission {
        allow: Vec::new(),
        deny: vec![">".to_owned()],
    };
    UserPermissionLimits {
        permissions: Permissions {
            publish: deny_all.clone(),
            subscribe: deny_all,
            resp: None,
        },
        bearer_token: Some(true),
        ..UserPermissionLimits::default()
    }
}

/// Whether an account may use JetStream; only the accounts that hold streams get it.
#[derive(Clone, Copy)]
enum JetStreamAccess {
    Disabled,
    Enabled,
}

struct AccountJwt {
    account: String,
    jwt: String,
}

impl AccountJwt {
    fn issue(
        name: &str,
        account: &str,
        signers: &[&nkeys::KeyPair],
        authorization: Option<ExternalAuthorization>,
        jetstream: JetStreamAccess,
        operator_signing: &nkeys::KeyPair,
    ) -> Self {
        let mut claims = Account::new_claims(name.to_owned(), account.to_owned());
        if let JetStreamAccess::Enabled = jetstream {
            claims.nats.limits = Some(OperatorLimits {
                jetstream: Some(JetStreamLimits {
                    memory_storage: Some(NO_LIMIT),
                    disk_storage: Some(NO_LIMIT),
                    streams: Some(NO_LIMIT),
                    consumer: Some(NO_LIMIT),
                    ..JetStreamLimits::default()
                }),
                ..OperatorLimits::default()
            });
        }
        claims.nats.signing_keys = Some(
            signers
                .iter()
                .map(|signer| SigningKey {
                    key: signer.public_key(),
                    scope: None,
                })
                .collect(),
        );
        claims.nats.authorization = authorization;
        Self {
            account: account.to_owned(),
            jwt: claims.encode(operator_signing).expect("encode account jwt"),
        }
    }
}

struct OperatorFixture {
    operator_jwt: String,
    system_account: String,
    account_jwts: Vec<AccountJwt>,
    auth_account: String,
    auth_response_signing: nkeys::KeyPair,
    callout_xkey: nkeys::XKey,
    callout_service: NatsUser,
    sentinel: NatsUser,
    tenant_observer: NatsUser,
    platform_user: NatsUser,
}

impl OperatorFixture {
    fn generate(tenant: &TenantFixture) -> Self {
        let operator = nkeys::KeyPair::new_operator();
        let operator_signing = nkeys::KeyPair::new_operator();
        let system = nkeys::KeyPair::new_account().public_key();
        let auth = nkeys::KeyPair::new_account().public_key();
        let platform = nkeys::KeyPair::new_account().public_key();
        let system_signing = nkeys::KeyPair::new_account();
        let auth_user_issuer = nkeys::KeyPair::new_account();
        let auth_response_signing = nkeys::KeyPair::new_account();
        let platform_signing = nkeys::KeyPair::new_account();
        let callout_xkey = nkeys::XKey::new();

        let callout_service = NatsUser::issue(
            "auth-callout",
            &auth,
            &auth_user_issuer,
            UserPermissionLimits::default(),
        );
        let sentinel = NatsUser::issue("sentinel", &auth, &auth_user_issuer, sentinel_permissions());
        let tenant_observer = NatsUser::issue(
            "observer",
            tenant.account.as_str(),
            &tenant.user_issuer,
            UserPermissionLimits::default(),
        );

        let platform_user = NatsUser::issue(
            "platform-reader",
            &platform,
            &platform_signing,
            UserPermissionLimits::default(),
        );

        let callout = ExternalAuthorization {
            auth_users: Some(BTreeSet::from([callout_service.public_key()])),
            allowed_accounts: Some(BTreeSet::from([tenant.name()])),
            xkey: Some(callout_xkey.public_key()),
        };
        let account_jwts = vec![
            AccountJwt::issue(
                "SYS",
                &system,
                &[&system_signing],
                None,
                JetStreamAccess::Disabled,
                &operator_signing,
            ),
            AccountJwt::issue(
                "AUTH",
                &auth,
                &[&auth_user_issuer, &auth_response_signing],
                Some(callout),
                JetStreamAccess::Disabled,
                &operator_signing,
            ),
            AccountJwt::issue(
                "PLATFORM",
                &platform,
                &[&platform_signing],
                None,
                JetStreamAccess::Enabled,
                &operator_signing,
            ),
            AccountJwt::issue(
                "TENANT",
                tenant.account.as_str(),
                &[&tenant.user_issuer, &tenant.callout_signing],
                None,
                JetStreamAccess::Enabled,
                &operator_signing,
            ),
        ];

        let mut operator_claims = Operator::new_claims("a2a-callout-it".to_owned(), operator.public_key());
        operator_claims.nats.signing_keys = Some(vec![operator_signing.public_key()]);
        operator_claims.nats.system_account = Some(system.clone());
        operator_claims.nats.strict_signing_key_usage = Some(true);

        Self {
            operator_jwt: operator_claims.encode(&operator).expect("encode operator jwt"),
            system_account: system,
            account_jwts,
            auth_account: auth,
            auth_response_signing,
            callout_xkey,
            callout_service,
            sentinel,
            tenant_observer,
            platform_user,
        }
    }

    fn wire_codec(&self) -> AuthCalloutWireCodec {
        let response_seed = self.auth_response_signing.seed().expect("auth response signing seed");
        let xkey_seed = self.callout_xkey.seed().expect("callout xkey seed");
        AuthCalloutWireCodec::new(
            AuthAccount::parse(&self.auth_account).expect("auth account"),
            AccountSigningKey::from_seed(&response_seed).expect("auth response signing key"),
            Some(CalloutXkey::from_seed(&xkey_seed).expect("callout xkey")),
        )
    }

    fn server_config(&self) -> String {
        let preload: String = self
            .account_jwts
            .iter()
            .map(|account| format!("  {}: \"{}\"\n", account.account, account.jwt))
            .collect();
        format!(
            r#"port: {NATS_CLIENT_PORT}
server_name: a2a-callout-it

tls {{
  cert_file: "{NATS_CONFIG_ROOT}/certs/server.pem"
  key_file: "{NATS_CONFIG_ROOT}/certs/server-key.pem"
  ca_file: "{NATS_CONFIG_ROOT}/certs/ca.pem"
  verify: true
  min_version: "1.3"
  timeout: 2
}}

operator: "{NATS_CONFIG_ROOT}/operator.jwt"
system_account: {system_account}

resolver {{
  type: full
  dir: "/data/jwt"
  allow_delete: false
  interval: "2m"
  timeout: "1.9s"
}}
resolver_preload: {{
{preload}}}

jetstream {{
  store_dir: "/data/jetstream"
}}

default_sentinel: "{sentinel}"
"#,
            system_account = self.system_account,
            sentinel = self.sentinel.jwt,
        )
    }

    fn write_into(&self, root: &Path) {
        std::fs::write(root.join("operator.jwt"), &self.operator_jwt).expect("write operator jwt");
        std::fs::write(root.join("nats.conf"), self.server_config()).expect("write nats.conf");
    }
}

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
        self.0.lock().expect("dispatch error lock").push(message.into());
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
                self.capture.0.lock().expect("mint capture lock").push(minted.clone());
                Ok(minted)
            }
            Err(err) => {
                self.errors.push(err.to_string());
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

impl NatsClientTls {
    fn apply(&self, options: ConnectOptions) -> ConnectOptions {
        options
            .add_root_certificates(self.ca_cert.clone())
            .add_client_certificate(self.client_cert.clone(), self.client_key.clone())
    }

    fn certificate_pem(&self) -> String {
        std::fs::read_to_string(&self.client_cert).expect("client certificate pem")
    }
}

struct TlsFixture {
    dir: TempDir,
    trust_anchor_pem: String,
    client: NatsClientTls,
}

impl TlsFixture {
    fn generate() -> Self {
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
        let issuer = Issuer::from_params(&ca_params, &ca_key);

        let server_key = KeyPair::generate().expect("server key");
        let mut server_dn = DistinguishedName::new();
        server_dn.push(DnType::CommonName, "nats-integration");
        let mut server_params = CertificateParams::default();
        server_params.distinguished_name = server_dn;
        server_params.subject_alt_names = vec![
            SanType::DnsName("localhost".try_into().expect("dns san")),
            SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        ];
        let server_cert = server_params.signed_by(&server_key, &issuer).expect("server cert");

        let ee_key = KeyPair::generate().expect("ee key");
        let mut ee_dn = DistinguishedName::new();
        ee_dn.push(DnType::CommonName, "integration-client");
        let mut ee_params = CertificateParams::default();
        ee_params.distinguished_name = ee_dn;
        let ee = ee_params.signed_by(&ee_key, &issuer).expect("ee cert");

        std::fs::write(cert_dir.join("server.pem"), server_cert.pem()).expect("server pem");
        std::fs::write(cert_dir.join("server-key.pem"), server_key.serialize_pem()).expect("server key");
        std::fs::write(cert_dir.join("ca.pem"), &ca_pem).expect("ca pem");
        std::fs::write(cert_dir.join("client.pem"), ee.pem()).expect("client pem");
        std::fs::write(cert_dir.join("client-key.pem"), ee_key.serialize_pem()).expect("client key");

        Self {
            client: NatsClientTls {
                ca_cert: cert_dir.join("ca.pem"),
                client_cert: cert_dir.join("client.pem"),
                client_key: cert_dir.join("client-key.pem"),
            },
            dir,
            trust_anchor_pem: ca_pem,
        }
    }
}

#[derive(Clone)]
enum CallerCredential {
    BearerTokenBehindTheSentinel { sentinel: String, bearer: String },
    ApiKeyToken(String),
    ClientCertificateOnly,
}

#[derive(Clone)]
struct Caller {
    tenant: TenantAccount,
    credential: CallerCredential,
}

impl Caller {
    fn auth(&self) -> Auth {
        let mut auth = Auth::new();
        auth.username = Some(self.tenant.as_str().to_owned());
        match &self.credential {
            CallerCredential::BearerTokenBehindTheSentinel { sentinel, bearer } => {
                auth.jwt = Some(sentinel.clone());
                auth.password = Some(bearer.clone());
            }
            CallerCredential::ApiKeyToken(key) => auth.token = Some(key.clone()),
            CallerCredential::ClientCertificateOnly => {}
        }
        auth
    }

    fn connect_options(&self, tls: &NatsClientTls) -> ConnectOptions {
        let caller = self.clone();
        tls.apply(ConnectOptions::with_auth_callback(move |_nonce| {
            let auth = caller.auth();
            async move { Ok(auth) }
        }))
    }
}

fn probe_agent() -> A2aAgentId {
    A2aAgentId::new("it").expect("probe agent id")
}

fn someone_else() -> SubjectPrincipal {
    let id = PrincipalId::new("someone-else").expect("principal id");
    SubjectPrincipal::from_credential(PrincipalKind::Service, &id).expect("subject principal")
}

struct SubscribeProbe {
    pattern: String,
    delivered: String,
}

impl SubscribeProbe {
    fn task_events(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        let task = A2aTaskId::new("task-1").expect("probe task id");
        Self {
            pattern: CallerTaskEventsSubject::new(prefix, caller).to_string(),
            delivered: TaskEventsSubject::new(prefix, &task, caller).to_string(),
        }
    }

    fn gateway_events(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        let events = GatewayEgressEventsSubject::new(prefix, caller).to_string();
        Self {
            pattern: events.clone(),
            delivered: events,
        }
    }
}

struct GrantProbe {
    request_inside: String,
    publish_outside: Vec<String>,
    subscribe_inside: Vec<SubscribeProbe>,
    subscribe_outside: Vec<String>,
}

impl GrantProbe {
    fn direct(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        let (agent, other) = (probe_agent(), someone_else());
        Self {
            request_inside: MessageSendSubject::new(prefix, &agent, caller).to_string(),
            publish_outside: vec![
                GatewayIngressSubject::new(prefix, &agent, caller, A2aMethod::MessageSend).to_string(),
                MessageSendSubject::new(prefix, &agent, &other).to_string(),
            ],
            subscribe_inside: vec![SubscribeProbe::task_events(prefix, caller)],
            subscribe_outside: vec![
                GatewayEgressEventsSubject::new(prefix, caller).to_string(),
                CallerTaskEventsSubject::new(prefix, &other).to_string(),
                "_INBOX.>".to_owned(),
            ],
        }
    }

    fn bridge_routed(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        let (agent, other) = (probe_agent(), someone_else());
        Self {
            request_inside: GatewayIngressSubject::new(prefix, &agent, caller, A2aMethod::MessageSend).to_string(),
            publish_outside: vec![
                MessageSendSubject::new(prefix, &agent, caller).to_string(),
                GatewayIngressSubject::new(prefix, &agent, &other, A2aMethod::MessageSend).to_string(),
            ],
            subscribe_inside: vec![
                SubscribeProbe::gateway_events(prefix, caller),
                SubscribeProbe::task_events(prefix, caller),
            ],
            subscribe_outside: vec![
                GatewayEgressEventsSubject::new(prefix, &other).to_string(),
                CallerTaskEventsSubject::new(prefix, &other).to_string(),
            ],
        }
    }

    fn refusals(&self) -> BTreeSet<String> {
        let publish = self
            .publish_outside
            .iter()
            .map(|subject| format!("Permissions Violation for Publish to \"{subject}\""));
        let subscribe = self
            .subscribe_outside
            .iter()
            .map(|subject| format!("Permissions Violation for Subscription to \"{subject}\""));
        publish.chain(subscribe).collect()
    }
}

struct NatsCalloutStack {
    _nats_container: ContainerAsync<GenericImage>,
    _config_dir: TempDir,
    nats_url: NatsClientUrl,
    client_tls: NatsClientTls,
    sentinel_jwt: String,
    tenant_observer: NatsUser,
    platform_user: NatsUser,
    capture: MintedJwtCapture,
    dispatch_errors: DispatchErrorCapture,
    signing: Arc<dyn SigningKeySource>,
    _subscriber_task: tokio::task::JoinHandle<()>,
}

impl NatsCalloutStack {
    async fn start(tenant: &TenantFixture, tls: TlsFixture, dispatcher: CalloutDispatcher) -> Self {
        trogon_std::tls::install_default_crypto_provider();
        let operator = OperatorFixture::generate(tenant);
        operator.write_into(tls.dir.path());

        let image = GenericImage::new(NATS_IMAGE, NATS_TAG)
            .with_exposed_port(NATS_CLIENT_PORT.tcp())
            .with_wait_for(WaitFor::message_on_either_std("Server is ready"))
            .with_startup_timeout(Duration::from_secs(120))
            .with_cmd(["-c".to_owned(), format!("{NATS_CONFIG_ROOT}/nats.conf")])
            .with_mount(Mount::bind_mount(
                tls.dir.path().to_str().expect("utf8 config path"),
                NATS_CONFIG_ROOT,
            ));

        let container = image.start().await.expect("start nats-server container");
        let host = container.get_host().await.expect("container host");
        let port = container
            .get_host_port_ipv4(NATS_CLIENT_PORT.tcp())
            .await
            .expect("mapped client port");
        let nats_url = NatsClientUrl(format!("tls://{host}:{port}"));

        let wire = operator.wire_codec();
        let capture = MintedJwtCapture::new();
        let dispatch_errors = DispatchErrorCapture::default();
        let capturing = CapturingDispatcher {
            inner: dispatcher,
            capture: capture.clone(),
            errors: dispatch_errors.clone(),
        };

        let callout_client = tls
            .client
            .apply(operator.callout_service.connect_options())
            .connect(nats_url.as_str())
            .await
            .unwrap_or_else(|err| panic!("callout service connect to {}: {err:?}", nats_url.as_str()));
        let mut requests = callout_client
            .subscribe(AUTH_CALLOUT_SUBJECT)
            .await
            .expect("subscribe to the auth callout subject");
        callout_client.flush().await.expect("callout service flush");
        let errors = dispatch_errors.clone();
        let subscriber_task = tokio::spawn(async move {
            while let Some(msg) = requests.next().await {
                let Some(reply) = msg.reply else {
                    errors.push("auth callout request without reply subject");
                    continue;
                };
                let request = match wire.decode_request(msg.payload.to_vec(), msg.headers.as_ref()) {
                    Ok(request) => request,
                    Err(err) => {
                        errors.push(format!("decode auth request: {err}"));
                        continue;
                    }
                };
                let response = match capturing.dispatch(request.clone()).await {
                    Ok(minted) => wire.encode_success(&request, minted),
                    Err(err) => wire.encode_denial(&request, DenialCategory::from_auth_callout_error(&err).as_str()),
                };
                match response {
                    Ok(payload) => {
                        if let Err(err) = callout_client.publish(reply, payload.into()).await {
                            errors.push(format!("publish auth response: {err}"));
                        }
                    }
                    Err(err) => errors.push(format!("encode auth response: {err}")),
                }
            }
        });

        Self {
            _nats_container: container,
            _config_dir: tls.dir,
            nats_url,
            client_tls: tls.client,
            sentinel_jwt: operator.sentinel.jwt,
            tenant_observer: operator.tenant_observer,
            platform_user: operator.platform_user,
            capture,
            dispatch_errors,
            signing: tenant.verification_source(),
            _subscriber_task: subscriber_task,
        }
    }

    async fn drive_callout_attempts(&self, caller: &Caller) {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline && self.capture.len() < 2 {
            let _ = caller
                .connect_options(&self.client_tls)
                .connect(self.nats_url.as_str())
                .await;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn assert_admits_with_only_the_minted_grant(
        &self,
        caller: &Caller,
        tenant: &TenantFixture,
        kind: PrincipalKind,
    ) {
        self.drive_callout_attempts(caller).await;
        let errors = self.dispatch_errors.drain();
        let principal = assert_stable_caller_id(&self.capture.drain(), self.signing.as_ref(), tenant, &errors, kind);
        self.assert_server_enforces_the_grant(
            caller
                .connect_options(&self.client_tls)
                .custom_inbox_prefix(format!("_INBOX.{principal}")),
            &GrantProbe::direct(&grant_prefix(), &principal),
        )
        .await;
    }

    async fn assert_bridge_routed_caller_holds_only_its_grant(
        &self,
        bridge: &CalloutDispatcher,
        tenant: &TenantFixture,
    ) {
        let user = nkeys::KeyPair::new_user();
        let request = ServerAuthRequestClaims::from_bridge_mint(
            BridgeMintRequest {
                user_nkey: Some(user.public_key()),
                account: Some(tenant.name()),
                client_info: Some(BridgeClientInfo {
                    client_cert_pem: Some(self.client_tls.certificate_pem()),
                }),
                sealed_credential: None,
            },
            None,
        )
        .expect("bridge mint request");
        let minted = bridge
            .dispatch(request)
            .await
            .expect("the callout mints the bridge-routed caller");
        assert_minted_nats_user_jwt_shape(minted.as_str(), tenant);
        let principal = subject_principal_from_minted_jwt(minted.as_str()).expect("the minted user names a principal");
        assert_eq!(principal.kind(), PrincipalKind::Service);
        ExpectedGrant::bridge_routed(&principal)
            .assert_minted(&decode_captured(&minted, self.signing.as_ref()), tenant);

        let caller = NatsUser {
            jwt: minted.as_str().to_owned(),
            key: Arc::new(user),
        };
        self.assert_server_enforces_the_grant(
            self.client_tls.apply(caller.connect_options()),
            &GrantProbe::bridge_routed(&grant_prefix(), &principal),
        )
        .await;
    }

    async fn assert_server_enforces_the_grant(&self, caller: ConnectOptions, probe: &GrantProbe) {
        let observer = self
            .client_tls
            .apply(self.tenant_observer.connect_options())
            .connect(self.nats_url.as_str())
            .await
            .expect("tenant observer connects with its own user JWT");
        let mut observed = observer
            .subscribe(TENANT_OBSERVED_SUBJECTS)
            .await
            .expect("tenant observer subscribes");
        observer.flush().await.expect("tenant observer flush");

        let (events_tx, mut events) = tokio::sync::mpsc::channel(64);
        let client = caller
            .event_callback(move |event| {
                let _ = events_tx.try_send(event);
                async {}
            })
            .connect(self.nats_url.as_str())
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "the callout admits the caller: {err:?}; dispatch errors: {:?}",
                    self.dispatch_errors.drain()
                )
            });

        for subject in &probe.publish_outside {
            client
                .publish(subject.clone(), "outside".into())
                .await
                .expect("publish outside the grant");
        }
        for subject in &probe.subscribe_outside {
            let _refused = client
                .subscribe(subject.clone())
                .await
                .expect("subscribe outside the grant");
        }
        client.flush().await.expect("caller flush");

        let (reply, ()) = tokio::join!(client.request(probe.request_inside.clone(), "inside".into()), async {
            let request = tokio::time::timeout(Duration::from_secs(5), observed.next())
                .await
                .expect("the tenant observer receives the request inside the grant")
                .expect("tenant observer subscription stays open");
            assert_eq!(request.subject.as_str(), probe.request_inside);
            let inbox = request.reply.expect("the request names a reply inbox");
            observer.publish(inbox, "reply".into()).await.expect("observer replies");
            observer.flush().await.expect("observer flush");
        });
        let reply = reply.expect("the caller receives the reply on an inbox it may read");
        assert_eq!(reply.payload, "reply");

        for inside in &probe.subscribe_inside {
            let mut subscription = client
                .subscribe(inside.pattern.clone())
                .await
                .expect("subscribe inside the grant");
            client.flush().await.expect("caller flush");
            let delivery = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    observer
                        .publish(inside.delivered.clone(), "event".into())
                        .await
                        .expect("observer publishes inside the grant");
                    observer.flush().await.expect("observer flush");
                    if let Ok(delivered) = tokio::time::timeout(Duration::from_millis(200), subscription.next()).await {
                        return delivered;
                    }
                }
            })
            .await;
            let Ok(delivered) = delivery else {
                panic!(
                    "{} never reached the caller through {}; the caller saw {:?}",
                    inside.delivered,
                    inside.pattern,
                    drain_events(&mut events)
                );
            };
            let delivered = delivered.expect("caller subscription stays open");
            assert_eq!(delivered.subject.as_str(), inside.delivered);
        }

        let mut unrefused = probe.refusals();
        let mut reported = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while !unrefused.is_empty() {
                match events.recv().await {
                    Some(Event::ServerError(ServerError::Other(message))) => {
                        unrefused.remove(&message);
                        reported.push(message);
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await;
        assert!(
            unrefused.is_empty(),
            "nats-server did not refuse {unrefused:?}; it reported {reported:?}"
        );
    }

    async fn connect_as(&self, user: &NatsUser) -> async_nats::Client {
        self.client_tls
            .apply(
                user.connect_options()
                    .request_timeout(Some(INVALIDATION_REQUEST_TIMEOUT)),
            )
            .connect(self.nats_url.as_str())
            .await
            .expect("connect with the user's own JWT")
    }

    /// Provisions the stream as `provision-secrets-invalidation.sh` shapes it, then
    /// proves a PLATFORM user reads it and receives a published notice.
    async fn assert_platform_holds_the_invalidation_stream(&self, platform: &async_nats::Client, notice_subject: &str) {
        let config = serde_json::json!({
            "name": INVALIDATION_STREAM,
            "subjects": [INVALIDATION_SUBJECTS],
            "storage": "file",
            "retention": "limits",
            "discard": "old",
            "num_replicas": 1,
            "duplicate_window": 120_000_000_000_i64,
            "max_age": 3_600_000_000_000_i64,
            "max_msgs": -1,
            "max_bytes": -1,
            "max_msg_size": 1024,
            "max_msgs_per_subject": -1,
            "deny_delete": true,
            "deny_purge": true,
            "allow_rollup_hdrs": false,
            "allow_direct": true,
        });
        let created = retry_js_api_until_enabled(
            platform,
            &format!("$JS.API.STREAM.CREATE.{INVALIDATION_STREAM}"),
            config.to_string(),
        )
        .await;
        assert_eq!(
            created["config"]["name"], INVALIDATION_STREAM,
            "create reply: {created}"
        );

        let info = js_api(platform, &format!("$JS.API.STREAM.INFO.{INVALIDATION_STREAM}"), "").await;
        assert_eq!(
            info["config"]["name"], INVALIDATION_STREAM,
            "PLATFORM stream info: {info}"
        );
        assert_eq!(info["state"]["messages"], 0, "PLATFORM stream info: {info}");

        let mut notices = platform
            .subscribe(INVALIDATION_WATCH)
            .await
            .expect("PLATFORM subscribes to the notices");
        platform.flush().await.expect("PLATFORM flush");
        let ack = js_api(platform, notice_subject, "release").await;
        assert_eq!(ack["stream"], INVALIDATION_STREAM, "publish ack: {ack}");
        assert_eq!(ack["seq"], 1, "publish ack: {ack}");
        let notice = tokio::time::timeout(Duration::from_secs(5), notices.next())
            .await
            .expect("PLATFORM receives the notice")
            .expect("PLATFORM subscription stays open");
        assert_eq!(notice.subject.as_str(), notice_subject);
    }

    /// Publishes a second notice and proves a tenant account member subscribed to the
    /// same subjects never sees either notice.
    async fn assert_tenant_member_sees_no_notice(&self, platform: &async_nats::Client, notice_subject: &str) {
        let member = self.connect_as(&self.tenant_observer).await;
        let mut watch = member
            .subscribe(INVALIDATION_WATCH)
            .await
            .expect("tenant member subscribes");
        member.flush().await.expect("tenant member flush");

        let ack = js_api(platform, notice_subject, "release").await;
        assert_eq!(ack["seq"], 2, "the PLATFORM publish happened: {ack}");
        platform.flush().await.expect("PLATFORM flush");
        member.flush().await.expect("tenant member flush after the publish");
        let delivered = tokio::time::timeout(Duration::from_secs(2), watch.next()).await;
        assert!(
            delivered.is_err(),
            "a tenant subscription received a PLATFORM notice: {delivered:?}"
        );
    }

    /// A tenant user with unrestricted permissions still gets its own account's
    /// JetStream, which has no such stream.
    async fn assert_tenant_jetstream_has_no_invalidation_stream(&self) {
        let member = self.connect_as(&self.tenant_observer).await;
        let info = js_api(&member, &format!("$JS.API.STREAM.INFO.{INVALIDATION_STREAM}"), "").await;
        assert_not_found(&info);
        let consumer = serde_json::json!({
            "stream_name": INVALIDATION_STREAM,
            "config": {"durable_name": "tenant-reader", "ack_policy": "explicit"},
        });
        let created = js_api(
            &member,
            &format!("$JS.API.CONSUMER.DURABLE.CREATE.{INVALIDATION_STREAM}.tenant-reader"),
            &consumer.to_string(),
        )
        .await;
        assert_not_found(&created);
    }

    /// A callout-minted tenant user holds no `$JS.API` or notice grant, so the server
    /// refuses each attempt and nothing answers it.
    async fn assert_minted_tenant_user_is_refused_the_invalidation_stream(&self, caller: &Caller) {
        let (events_tx, mut events) = tokio::sync::mpsc::channel(64);
        let client = caller
            .connect_options(&self.client_tls)
            .request_timeout(Some(INVALIDATION_REQUEST_TIMEOUT))
            .event_callback(move |event| {
                let _ = events_tx.try_send(event);
                async {}
            })
            .connect(self.nats_url.as_str())
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "the callout admits the caller: {err:?}; dispatch errors: {:?}",
                    self.dispatch_errors.drain()
                )
            });

        let info_subject = format!("$JS.API.STREAM.INFO.{INVALIDATION_STREAM}");
        let consumer_subject = format!("$JS.API.CONSUMER.DURABLE.CREATE.{INVALIDATION_STREAM}.minted-reader");
        let info = client.request(info_subject.clone(), "".into()).await;
        assert!(
            info.is_err(),
            "the minted tenant user got a stream info reply: {info:?}"
        );
        let consumer = client
            .request(
                consumer_subject.clone(),
                serde_json::json!({
                    "stream_name": INVALIDATION_STREAM,
                    "config": {"durable_name": "minted-reader", "ack_policy": "explicit"},
                })
                .to_string()
                .into(),
            )
            .await;
        assert!(
            consumer.is_err(),
            "the minted tenant user got a consumer create reply: {consumer:?}"
        );
        let mut watch = client
            .subscribe(INVALIDATION_WATCH)
            .await
            .expect("subscribe outside the grant");
        client.flush().await.expect("caller flush");
        let delivered = tokio::time::timeout(Duration::from_secs(1), watch.next()).await;
        assert!(
            delivered.is_err(),
            "the minted tenant user received a notice: {delivered:?}"
        );

        let mut unrefused: BTreeSet<String> = [
            format!("Permissions Violation for Publish to \"{info_subject}\""),
            format!("Permissions Violation for Publish to \"{consumer_subject}\""),
            format!("Permissions Violation for Subscription to \"{INVALIDATION_WATCH}\""),
        ]
        .into();
        let mut reported = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while !unrefused.is_empty() {
                match events.recv().await {
                    Some(Event::ServerError(ServerError::Other(message))) => {
                        unrefused.remove(&message);
                        reported.push(message);
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await;
        assert!(
            unrefused.is_empty(),
            "nats-server did not refuse {unrefused:?}; it reported {reported:?}"
        );
    }

    async fn assert_tenant_cannot_reach_the_invalidation_stream(&self, caller: &Caller, tenant: &TenantFixture) {
        // The real shape is secrets.invalidation.v1.{tenant}.{owner_token}.{credential_id}, and
        // this notice names the tenant account the isolation is checked against.
        let notice_subject = format!("secrets.invalidation.v1.{}.owner~1.cred-1", tenant.name());
        let platform = self.connect_as(&self.platform_user).await;
        self.assert_platform_holds_the_invalidation_stream(&platform, &notice_subject)
            .await;
        self.assert_tenant_member_sees_no_notice(&platform, &notice_subject)
            .await;
        self.assert_tenant_jetstream_has_no_invalidation_stream().await;
        self.assert_minted_tenant_user_is_refused_the_invalidation_stream(caller)
            .await;

        let info = js_api(&platform, &format!("$JS.API.STREAM.INFO.{INVALIDATION_STREAM}"), "").await;
        assert_eq!(info["state"]["messages"], 2, "PLATFORM stream info: {info}");
        assert_eq!(
            info["state"]["consumer_count"], 0,
            "a tenant user created a consumer on the PLATFORM stream: {info}"
        );
    }

    async fn assert_refuses(&self, caller: &Caller) {
        let attempt = caller
            .connect_options(&self.client_tls)
            .connect(self.nats_url.as_str())
            .await;
        assert!(attempt.is_err(), "nats-server admitted a caller the callout refused");
    }
}

const INVALIDATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Sends a JetStream API request or a JetStream publish and parses the JSON reply.
///
/// The failure message names no subject, since a notice subject is built from a
/// generated account key; the caller's line says which request failed.
async fn js_api(client: &async_nats::Client, subject: &str, payload: &str) -> serde_json::Value {
    let reply = client
        .request(subject.to_owned(), payload.to_owned().into())
        .await
        .unwrap_or_else(|err| panic!("a jetstream request got no reply: {err:?}"));
    serde_json::from_slice(&reply.payload).expect("jetstream replies with json")
}

/// JetStream finishes enabling a preloaded account shortly after the server starts, so
/// the first requests can find nobody listening.
async fn retry_js_api_until_enabled(client: &async_nats::Client, subject: &str, payload: String) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match client.request(subject.to_owned(), payload.clone().into()).await {
            Ok(reply) => return serde_json::from_slice(&reply.payload).expect("jetstream replies with json"),
            Err(err) if std::time::Instant::now() >= deadline => panic!("{subject} got no reply: {err:?}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
}

fn assert_not_found(reply: &serde_json::Value) {
    assert_eq!(
        reply["error"]["err_code"], JS_STREAM_NOT_FOUND,
        "the tenant account answered with something other than stream not found: {reply}"
    );
    assert!(
        reply.get("config").is_none() && reply.get("state").is_none(),
        "PLATFORM stream details reached a tenant user: {reply}"
    );
}

fn drain_events(events: &mut tokio::sync::mpsc::Receiver<Event>) -> Vec<String> {
    std::iter::from_fn(|| events.try_recv().ok())
        .map(|event| event.to_string())
        .collect()
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
    let encoding_key = EncodingKey::from_rsa_pem(key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).expect("pem").as_bytes())
        .expect("encoding key");
    let public = key.to_public_key();
    let n = b64url_uint_be(&public.n().to_bytes_be());
    let e = b64url_uint_be(&public.e().to_bytes_be());
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_operations: Some(vec![KeyOperations::Verify]),
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
    let issuer = MockOidcIssuerUrl("https://issuer.example".to_owned());
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

fn decode_captured(minted: &MintedUserJwt, signing: &dyn SigningKeySource) -> UserJwtClaims {
    UserJwtClaims::verify_with_source(minted.as_str(), signing).expect("decode minted user jwt")
}

fn assert_minted_nats_user_jwt_shape(token: &str, tenant: &TenantFixture) {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
    assert_eq!(header["alg"], "ed25519-nkey");
    let payload = decode_nats_user_payload(token).expect("payload");
    assert_eq!(payload["nats"]["type"], "user");
    assert_eq!(payload["aud"], tenant.account.as_str());
    assert_eq!(payload["iss"], tenant.callout_signing.public_key().as_str());
    assert_eq!(payload["nats"]["issuer_account"], tenant.account.as_str());
    assert!(!payload["caller_id"].as_str().unwrap_or("").is_empty());
}

struct ExpectedGrant {
    publish: Vec<String>,
    subscribe: Vec<String>,
}

impl ExpectedGrant {
    fn direct(caller: &SubjectPrincipal) -> Self {
        Self {
            publish: vec![format!("{A2A_PREFIX}.v1.agents.*.{caller}.>")],
            subscribe: vec![
                format!("{A2A_PREFIX}.v1.tasks.*.{caller}.events"),
                format!("_INBOX.{caller}.>"),
            ],
        }
    }

    fn bridge_routed(caller: &SubjectPrincipal) -> Self {
        Self {
            publish: vec![format!("{A2A_PREFIX}.v1.gateway.*.{caller}.>")],
            subscribe: vec![
                format!("{A2A_PREFIX}.v1.gateway.{caller}.events"),
                format!("{A2A_PREFIX}.v1.tasks.*.{caller}.events"),
                "_INBOX.>".to_owned(),
            ],
        }
    }

    fn assert_minted(&self, claims: &UserJwtClaims, tenant: &TenantFixture) {
        let rendered = |patterns: &[SubjectPattern]| patterns.iter().map(|p| p.as_str().to_owned()).collect::<Vec<_>>();
        assert_eq!(claims.aud.as_str(), tenant.account.as_str());
        assert_eq!(rendered(&claims.nats_permissions.publish_allow), self.publish);
        assert_eq!(rendered(&claims.nats_permissions.subscribe_allow), self.subscribe);
    }
}

fn assert_stable_caller_id(
    minted: &[MintedUserJwt],
    signing: &dyn SigningKeySource,
    tenant: &TenantFixture,
    dispatch_errors: &[String],
    kind: PrincipalKind,
) -> SubjectPrincipal {
    assert!(
        minted.len() >= 2,
        "expected at least two callout mints, got {}; decode/dispatch errors: {dispatch_errors:?}",
        minted.len()
    );
    let first = decode_captured(&minted[0], signing);
    let second = decode_captured(&minted[1], signing);
    assert_eq!(first.caller_id, second.caller_id);
    assert_minted_nats_user_jwt_shape(minted[0].as_str(), tenant);
    let principal = subject_principal_from_minted_jwt(minted[0].as_str()).expect("the minted user names a principal");
    assert_eq!(principal.kind(), kind);
    let grant = ExpectedGrant::direct(&principal);
    grant.assert_minted(&first, tenant);
    grant.assert_minted(&second, tenant);
    principal
}

fn grant_prefix() -> A2aPrefix {
    A2aPrefix::new(A2A_PREFIX).expect("a2a prefix")
}

fn dispatcher_with_oidc(tenant: &TenantFixture, issuer: OidcIssuerUrl, jwks: JwkSet) -> CalloutDispatcher {
    let membership = TenantMembership::for_oidc_subject(
        tenant.account.clone(),
        &issuer,
        &ExternalSubject::new("user-integration-oidc").unwrap(),
    )
    .unwrap();
    let oidc: Arc<dyn OidcVerifier> = Arc::new(JwksOidcVerifier::with_static_jwks(
        issuer,
        vec![OIDC_AUDIENCE.into()],
        jwks,
    ));
    CalloutDispatcher::new(CalloutDispatcherConfig {
        tenant_signing_keys: tenant.signing_keys(),
        tenant_admission: Arc::new(TenantMemberships::new([membership]).unwrap()),
        user_jwt_ttl: Duration::from_secs(300),
        a2a_prefix: grant_prefix(),
        oidc: Some(oidc),
        mtls: None,
        api_key: None,
    })
}

fn dispatcher_with_mtls(tenant: &TenantFixture, trust_anchor_pem: String) -> CalloutDispatcher {
    let membership = TenantMembership::for_legacy_mtls_subject(
        tenant.account.clone(),
        &ExternalSubject::new("CN=integration-client").unwrap(),
    )
    .unwrap();
    let mtls: Arc<dyn MTlsVerifier> = Arc::new(X509MtlsVerifier::new(TrustAnchorPem::new(trust_anchor_pem)));
    CalloutDispatcher::new(CalloutDispatcherConfig {
        tenant_signing_keys: tenant.signing_keys(),
        tenant_admission: Arc::new(TenantMemberships::new([membership]).unwrap()),
        user_jwt_ttl: Duration::from_secs(300),
        a2a_prefix: grant_prefix(),
        oidc: None,
        mtls: Some(mtls),
        api_key: None,
    })
}

#[allow(deprecated)]
fn dispatcher_with_api_key(tenant: &TenantFixture) -> CalloutDispatcher {
    let membership = TenantMembership::for_legacy_api_key_subject(
        tenant.account.clone(),
        &ExternalSubject::new("svc-integration").unwrap(),
    )
    .unwrap();
    let mut registry = ApiKeyRegistry::new(API_KEY_HMAC.to_vec());
    let key = ApiKey::new(API_KEY_VALUE).expect("api key");
    registry.register(
        &key,
        ApiKeyEntry {
            spicedb_principal: SpiceDbPrincipal::new("svc/integration"),
            audience: AudienceAccount::new(tenant.account.as_str()),
            external_subject: ExternalSubject::new("svc-integration").expect("external subject"),
        },
    );
    let api_key: Arc<dyn ApiKeyVerifier> = Arc::new(HmacApiKeyVerifier::new(Arc::new(registry)));
    CalloutDispatcher::new(CalloutDispatcherConfig {
        tenant_signing_keys: tenant.signing_keys(),
        tenant_admission: Arc::new(TenantMemberships::new([membership]).unwrap()),
        user_jwt_ttl: Duration::from_secs(300),
        a2a_prefix: grant_prefix(),
        oidc: None,
        mtls: None,
        api_key: Some(api_key),
    })
}

#[tokio::test]
async fn an_oidc_caller_behind_the_sentinel_lands_in_its_tenant_with_only_the_minted_grant() {
    let tenant = TenantFixture::generate();
    let (_oidc_mock, issuer_url, jwks, enc) = mock_oidc_issuer().await;
    let issuer = OidcIssuerUrl::parse(issuer_url.as_str()).expect("issuer url");
    let stack = NatsCalloutStack::start(
        &tenant,
        TlsFixture::generate(),
        dispatcher_with_oidc(&tenant, issuer, jwks),
    )
    .await;

    let caller = Caller {
        tenant: tenant.account.clone(),
        credential: CallerCredential::BearerTokenBehindTheSentinel {
            sentinel: stack.sentinel_jwt.clone(),
            bearer: mint_oidc_bearer(&issuer_url, &enc, "user-integration-oidc"),
        },
    };
    stack
        .assert_admits_with_only_the_minted_grant(&caller, &tenant, PrincipalKind::Person)
        .await;
}

#[tokio::test]
async fn a_client_certificate_caller_lands_in_its_tenant_with_only_the_minted_grant() {
    let tenant = TenantFixture::generate();
    let tls = TlsFixture::generate();
    let dispatcher = dispatcher_with_mtls(&tenant, tls.trust_anchor_pem.clone());
    let stack = NatsCalloutStack::start(&tenant, tls, dispatcher).await;

    let caller = Caller {
        tenant: tenant.account.clone(),
        credential: CallerCredential::ClientCertificateOnly,
    };
    stack
        .assert_admits_with_only_the_minted_grant(&caller, &tenant, PrincipalKind::Service)
        .await;
}

#[tokio::test]
async fn an_api_key_caller_lands_in_its_tenant_with_only_the_minted_grant() {
    let tenant = TenantFixture::generate();
    let stack = NatsCalloutStack::start(&tenant, TlsFixture::generate(), dispatcher_with_api_key(&tenant)).await;

    let caller = Caller {
        tenant: tenant.account.clone(),
        credential: CallerCredential::ApiKeyToken(API_KEY_VALUE.to_owned()),
    };
    stack
        .assert_admits_with_only_the_minted_grant(&caller, &tenant, PrincipalKind::Service)
        .await;

    let impostor = Caller {
        tenant: tenant.account.clone(),
        credential: CallerCredential::ApiKeyToken("k_live_unknown".to_owned()),
    };
    stack.assert_refuses(&impostor).await;
}

#[tokio::test]
async fn a_bridge_routed_caller_reaches_agents_only_through_the_gateway() {
    let tenant = TenantFixture::generate();
    let tls = TlsFixture::generate();
    let bridge = dispatcher_with_mtls(&tenant, tls.trust_anchor_pem.clone());
    let dispatcher = dispatcher_with_mtls(&tenant, tls.trust_anchor_pem.clone());
    let stack = NatsCalloutStack::start(&tenant, tls, dispatcher).await;

    stack
        .assert_bridge_routed_caller_holds_only_its_grant(&bridge, &tenant)
        .await;
}

#[tokio::test]
async fn a_callout_minted_tenant_user_cannot_reach_the_platform_invalidation_stream() {
    let tenant = TenantFixture::generate();
    let tls = TlsFixture::generate();
    let dispatcher = dispatcher_with_mtls(&tenant, tls.trust_anchor_pem.clone());
    let stack = NatsCalloutStack::start(&tenant, tls, dispatcher).await;

    let caller = Caller {
        tenant: tenant.account.clone(),
        credential: CallerCredential::ClientCertificateOnly,
    };
    stack
        .assert_tenant_cannot_reach_the_invalidation_stream(&caller, &tenant)
        .await;
}
