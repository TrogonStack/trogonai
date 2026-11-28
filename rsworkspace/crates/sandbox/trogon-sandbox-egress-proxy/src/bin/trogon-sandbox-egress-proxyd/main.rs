//! Runnable sidecar process for the transparent TCP egress proxy.
//!
//! Binds a listener, loads a [`NetworkPolicy`] once at startup, and for each
//! accepted connection recovers the connection's original (pre-redirect)
//! destination and hands it to [`trogon_sandbox_egress_proxy::proxy::handle_connection`].
//! Wiring the actual iptables/nftables REDIRECT rules that cause connections
//! to arrive here is done outside this binary.

use std::fs;
use std::io;
#[cfg(unix)]
use std::io::Read as _;
#[cfg(unix)]
use std::os::fd::FromRawFd as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::net::TcpListener;
use tracing::{info, warn};
use trogon_sandbox_contract::NetworkOptions;
use trogon_sandbox_contract::{
    CredentialProvider, DomainRuleAction, LeaseId, NetworkPolicy, PolicyFence, SandboxId, TenantId,
};
use trogon_sandbox_egress_proxy::credential_source::{
    BrokerCredentialSource, CredentialSource, UnconfiguredCredentialSource,
};
use trogon_sandbox_egress_proxy::decider::{
    Decider, DeciderLimits, LivePolicyAmender, NatsDeciderTransport, NatsPolicyPublisher,
};
use trogon_sandbox_egress_proxy::dial::{self, UpstreamExemption};
use trogon_sandbox_egress_proxy::ingress::{IngressLimits, ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::network_env;
use trogon_sandbox_egress_proxy::oidc_signer::OidcSigner;
use trogon_sandbox_egress_proxy::proxy::OidcIdentityContext;
use trogon_sandbox_egress_proxy::secret_substitution::SecretSubstitution;
use trogon_sandbox_egress_proxy::server::{
    OIDC_ISSUER, PolicyHandle, PolicyRejectedError, ProxyServer, permitted_domains_for,
};
use trogon_sandbox_egress_proxy::tls_ca::{SandboxCa, install_default_crypto_provider};
use trogon_sandbox_egress_proxy::violation::{NatsViolationSink, ViolationRateLimit, ViolationReporter};
#[cfg(unix)]
use trogon_sandbox_protocol::RegistryHandoff;
use trogon_sandbox_protocol::SentinelRegistry;
use trogon_std::env::{ReadEnv as _, SystemEnv};

mod constants;
mod env_encoding;
use constants::{
    CA_CERT_FILE_ENV_VAR, CA_KEY_FILE_ENV_VAR, CA_OUT_FILE_ENV_VAR, CREDENTIAL_BROKER_LEASE_ID_ENV_VAR,
    CREDENTIAL_BROKER_NATS_URL_ENV_VAR, CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR, CREDENTIAL_BROKER_TENANT_ID_ENV_VAR,
    DECIDER_MAX_IN_FLIGHT_ENV_VAR, DECIDER_TIMEOUT_MS_ENV_VAR, DEFAULT_LISTEN_ADDR, DEFAULT_MUX_LISTEN_ADDR,
    EXEMPT_UID_ENV_VAR, LISTEN_ADDR_ENV_VAR, MUX_LISTEN_ADDR_ENV_VAR, MUX_TOKEN_ENV_VAR, OIDC_LEASE_ID_ENV_VAR,
    OIDC_SIGNING_KEY_PEM_FILE_ENV_VAR, OIDC_SUBJECT_ENV_VAR, OIDC_TENANT_ID_ENV_VAR, POLICY_FILE_ENV_VAR,
    POLICY_RELOAD_POLL_INTERVAL, SECRETS_FD_ENV_VAR, TRANSPARENT_LISTENER_OFF, VIOLATION_LEASE_ID_ENV_VAR,
    VIOLATION_NATS_URL_ENV_VAR,
};

/// Fully resolved startup configuration for the proxy daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    listen_addr: Option<String>,
    policy_file: String,
    policy: NetworkPolicy,
    ca_out_file: Option<String>,
    supplied_ca: Option<(String, String)>,
    credential_broker: Option<CredentialBrokerConfig>,
    oidc_identity: Option<OidcIdentityConfig>,
    violation_events: Option<ViolationEventsConfig>,
    decider: Option<DeciderConfig>,
    mux: Option<MuxConfig>,
    secrets_fd: Option<i32>,
    upstream_exemption: UpstreamExemption,
    network_options: NetworkOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeciderConfig {
    nats_url: String,
    lease_id: LeaseId,
    limits: DeciderLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MuxConfig {
    listen_addr: ListenAddress,
    inherited_fd: Option<i32>,
    token: ProxyToken,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ListenFlags {
    listen_fd: Option<i32>,
    listen: Option<String>,
    any_interface: bool,
}

impl ListenFlags {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut flags = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--listen-fd" => {
                    let raw = args.next().ok_or("--listen-fd needs a descriptor number")?;
                    let fd = raw
                        .parse::<i32>()
                        .ok()
                        .filter(|fd| *fd > 2)
                        .ok_or_else(|| format!("--listen-fd \"{raw}\" is not an open descriptor number"))?;
                    flags.listen_fd = Some(fd);
                }
                "--listen" => {
                    flags.listen = Some(args.next().ok_or("--listen needs an address")?);
                }
                "--listen-any-interface" => flags.any_interface = true,
                other => return Err(format!("unknown argument \"{other}\"")),
            }
        }
        if flags.listen_fd.is_some() && flags.listen.is_some() {
            return Err("--listen-fd and --listen cannot be combined".to_string());
        }
        Ok(flags)
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn apply(&self, mut config: Config, token: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        if self.is_empty() {
            return Ok(config);
        }
        let mux_token = match &config.mux {
            Some(mux) => mux.token.clone(),
            None => {
                let raw = token(MUX_TOKEN_ENV_VAR).ok_or_else(|| {
                    format!("the listen flags need {MUX_TOKEN_ENV_VAR}, the proxy password clients present")
                })?;
                ProxyToken::new(&raw).map_err(|err| format!("{MUX_TOKEN_ENV_VAR} is unusable: {err}"))?
            }
        };
        let previous = config.mux.as_ref().map(|mux| mux.listen_addr.clone());
        let (listen_addr, inherited_fd) = match (&self.listen, self.listen_fd) {
            (Some(raw), _) => {
                let address = ListenAddress::parse(raw);
                if let ListenAddress::Tcp(addr) = &address
                    && !self.any_interface
                    && !is_loopback_listen(addr)
                {
                    return Err(format!(
                        "--listen \"{raw}\" is not a loopback address (127.0.0.0/8, [::1] or localhost); pass --listen-any-interface to bind it anyway"
                    ));
                }
                (address, None)
            }
            (None, Some(fd)) => (ListenAddress::Tcp(format!("fd:{fd}")), Some(fd)),
            (None, None) => (
                previous.unwrap_or_else(|| ListenAddress::Tcp(DEFAULT_MUX_LISTEN_ADDR.to_string())),
                None,
            ),
        };
        config.mux = Some(MuxConfig {
            listen_addr,
            inherited_fd,
            token: mux_token,
        });
        Ok(config)
    }
}

fn is_loopback_listen(addr: &str) -> bool {
    let host = match addr.rsplit_once(':') {
        Some((host, _)) => host.trim_start_matches('[').trim_end_matches(']'),
        None => addr,
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViolationEventsConfig {
    nats_url: String,
    lease_id: LeaseId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CredentialBrokerConfig {
    nats_url: String,
    tenant: TenantId,
    fence: Option<PolicyFence>,
}

/// Env-driven identity used to mint OIDC tokens for `Forward` domain rules;
/// see [`OIDC_TENANT_ID_ENV_VAR`] for why this is independent of
/// [`CredentialBrokerConfig`]. `signing_key_pem_file` is carried as the raw
/// path (not the parsed key) so a malformed key file is reported as a
/// startup error from [`run`] rather than from [`load_config`], mirroring
/// how `policy_file`'s *contents* are validated later than its *presence*.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OidcIdentityConfig {
    tenant_id: TenantId,
    lease_id: LeaseId,
    subject: SandboxId,
    signing_key_pem_file: Option<String>,
}

/// Every [`CredentialProvider`] referenced by a `Transform` domain rule in
/// `policy`, deduplicated, so the process connects exactly the credential
/// broker clients it will ever need and no more.
fn referenced_credential_providers(policy: &NetworkPolicy) -> Vec<CredentialProvider> {
    let NetworkPolicy::UserDefined { rules, .. } = policy else {
        return Vec::new();
    };

    let mut providers = Vec::new();
    for rule in rules {
        if let DomainRuleAction::Transform(transform) = &rule.action
            && !providers.contains(&transform.credential.provider)
        {
            providers.push(transform.credential.provider);
        }
    }
    providers
}

fn policy_has_forward_rule(policy: &NetworkPolicy) -> bool {
    let NetworkPolicy::UserDefined { rules, .. } = policy else {
        return false;
    };
    rules
        .iter()
        .any(|rule| matches!(rule.action, DomainRuleAction::Forward(_)))
}

/// Reads and parses environment-driven configuration.
///
/// The listen address comes from `EGRESS_PROXY_LISTEN_ADDR`, defaulting to
/// `0.0.0.0:3128` when unset. The network policy is read from the JSON file
/// path in `EGRESS_PROXY_POLICY_FILE`, which is required: a sidecar container
/// with no policy configured is not a safe default, so this fails fast
/// instead of silently falling back to allow-all or deny-all.
fn load_config(
    get_env: impl Fn(&str) -> Option<String>,
    read_file: impl Fn(&str) -> io::Result<String>,
) -> Result<Config, String> {
    let listen_addr = match get_env(LISTEN_ADDR_ENV_VAR).as_deref() {
        Some(TRANSPARENT_LISTENER_OFF) => None,
        Some(addr) => Some(addr.to_string()),
        None => Some(DEFAULT_LISTEN_ADDR.to_string()),
    };

    let policy_file = get_env(POLICY_FILE_ENV_VAR)
        .ok_or_else(|| format!("{POLICY_FILE_ENV_VAR} is required (path to a JSON file containing a NetworkPolicy)"))?;

    let policy_json = read_file(&policy_file)
        .map_err(|err| format!("failed to read policy file {POLICY_FILE_ENV_VAR}={policy_file}: {err}"))?;

    let policy: NetworkPolicy = serde_json::from_str(&policy_json)
        .map_err(|err| format!("failed to parse policy file {policy_file} as NetworkPolicy JSON: {err}"))?;

    let ca_out_file = get_env(CA_OUT_FILE_ENV_VAR);
    let supplied_ca = match (get_env(CA_CERT_FILE_ENV_VAR), get_env(CA_KEY_FILE_ENV_VAR)) {
        (Some(cert), Some(key)) => Some((cert, key)),
        (None, None) => None,
        _ => {
            return Err(format!(
                "{CA_CERT_FILE_ENV_VAR} and {CA_KEY_FILE_ENV_VAR} must be set together"
            ));
        }
    };

    let nats_url = get_env(CREDENTIAL_BROKER_NATS_URL_ENV_VAR);
    let tenant_id = get_env(CREDENTIAL_BROKER_TENANT_ID_ENV_VAR);
    let fence = match (
        get_env(CREDENTIAL_BROKER_LEASE_ID_ENV_VAR),
        get_env(CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR),
    ) {
        (Some(lease_id), Some(revision)) => Some(PolicyFence {
            lease_id: LeaseId::new(lease_id),
            policy_revision: revision
                .parse()
                .map_err(|_| format!("{CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR} must be an unsigned integer"))?,
        }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "{CREDENTIAL_BROKER_LEASE_ID_ENV_VAR} and {CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR} must be set together"
            ));
        }
    };
    let credential_broker = match (nats_url, tenant_id) {
        (Some(nats_url), Some(tenant_id)) => Some(CredentialBrokerConfig {
            nats_url,
            tenant: TenantId::new(tenant_id).map_err(|err| format!("{CREDENTIAL_BROKER_TENANT_ID_ENV_VAR}: {err}"))?,
            fence,
        }),
        (None, None) => None,
        (Some(_), None) => {
            return Err(format!(
                "{CREDENTIAL_BROKER_TENANT_ID_ENV_VAR} is required when {CREDENTIAL_BROKER_NATS_URL_ENV_VAR} is set"
            ));
        }
        (None, Some(_)) => {
            return Err(format!(
                "{CREDENTIAL_BROKER_NATS_URL_ENV_VAR} is required when {CREDENTIAL_BROKER_TENANT_ID_ENV_VAR} is set"
            ));
        }
    };

    let oidc_tenant_id = get_env(OIDC_TENANT_ID_ENV_VAR);
    let oidc_lease_id = get_env(OIDC_LEASE_ID_ENV_VAR);
    let oidc_subject = get_env(OIDC_SUBJECT_ENV_VAR);
    let oidc_identity = match (oidc_tenant_id, oidc_lease_id, oidc_subject) {
        (Some(tenant_id), Some(lease_id), Some(subject)) => Some(OidcIdentityConfig {
            tenant_id: TenantId::new(tenant_id).map_err(|err| format!("{OIDC_TENANT_ID_ENV_VAR}: {err}"))?,
            lease_id: LeaseId::new(lease_id),
            subject: SandboxId::new(subject).map_err(|err| format!("{OIDC_SUBJECT_ENV_VAR}: {err}"))?,
            signing_key_pem_file: get_env(OIDC_SIGNING_KEY_PEM_FILE_ENV_VAR),
        }),
        (None, None, None) => None,
        _ => {
            return Err(format!(
                "{OIDC_TENANT_ID_ENV_VAR}, {OIDC_LEASE_ID_ENV_VAR}, and {OIDC_SUBJECT_ENV_VAR} must all be set together (or all unset)"
            ));
        }
    };

    let violation_events = match (get_env(VIOLATION_NATS_URL_ENV_VAR), get_env(VIOLATION_LEASE_ID_ENV_VAR)) {
        (Some(nats_url), Some(lease_id)) => Some(ViolationEventsConfig {
            nats_url,
            lease_id: LeaseId::new(lease_id),
        }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "{VIOLATION_NATS_URL_ENV_VAR} and {VIOLATION_LEASE_ID_ENV_VAR} must both be set (or both unset)"
            ));
        }
    };

    let decider = match get_env(DECIDER_TIMEOUT_MS_ENV_VAR) {
        None => None,
        Some(raw_timeout) => {
            let Some(events) = &violation_events else {
                return Err(format!(
                    "{DECIDER_TIMEOUT_MS_ENV_VAR} needs {VIOLATION_NATS_URL_ENV_VAR} and {VIOLATION_LEASE_ID_ENV_VAR}"
                ));
            };
            let timeout_ms = raw_timeout
                .parse::<u64>()
                .ok()
                .filter(|ms| *ms > 0)
                .ok_or_else(|| format!("{DECIDER_TIMEOUT_MS_ENV_VAR} must be a positive number of milliseconds"))?;
            let defaults = DeciderLimits::default();
            let max_in_flight = match get_env(DECIDER_MAX_IN_FLIGHT_ENV_VAR) {
                None => defaults.max_in_flight,
                Some(raw) => raw
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("{DECIDER_MAX_IN_FLIGHT_ENV_VAR} must be a positive number"))?,
            };
            Some(DeciderConfig {
                nats_url: events.nats_url.clone(),
                lease_id: events.lease_id.clone(),
                limits: DeciderLimits {
                    timeout: Duration::from_millis(timeout_ms),
                    max_in_flight,
                },
            })
        }
    };

    let mux = match (get_env(MUX_LISTEN_ADDR_ENV_VAR), get_env(MUX_TOKEN_ENV_VAR)) {
        (Some(listen_addr), Some(token)) => Some(MuxConfig {
            listen_addr: ListenAddress::parse(&listen_addr),
            inherited_fd: None,
            token: ProxyToken::new(&token).map_err(|err| format!("{MUX_TOKEN_ENV_VAR} is unusable: {err}"))?,
        }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "{MUX_LISTEN_ADDR_ENV_VAR} and {MUX_TOKEN_ENV_VAR} must both be set (or both unset)"
            ));
        }
    };

    if listen_addr.is_none() && mux.is_none() {
        return Err(format!(
            "{LISTEN_ADDR_ENV_VAR}={TRANSPARENT_LISTENER_OFF} needs {MUX_LISTEN_ADDR_ENV_VAR} and {MUX_TOKEN_ENV_VAR}, otherwise nothing would be served"
        ));
    }

    let secrets_fd = match get_env(SECRETS_FD_ENV_VAR) {
        Some(raw) => Some(
            raw.parse::<i32>()
                .ok()
                .filter(|fd| *fd > 2)
                .ok_or_else(|| format!("{SECRETS_FD_ENV_VAR} must be an open descriptor number"))?,
        ),
        None => None,
    };

    let upstream_exemption = UpstreamExemption::from_exempt_uid(get_env(EXEMPT_UID_ENV_VAR).as_deref())
        .map_err(|err| format!("{EXEMPT_UID_ENV_VAR}: {err}"))?;

    let network_options = network_env::options_from_env(&get_env).map_err(|err| err.to_string())?;

    Ok(Config {
        listen_addr,
        policy_file,
        policy,
        ca_out_file,
        supplied_ca,
        credential_broker,
        oidc_identity,
        violation_events,
        decider,
        mux,
        secrets_fd,
        upstream_exemption,
        network_options,
    })
}

#[cfg(unix)]
fn read_secret_registry(fd: i32) -> io::Result<SentinelRegistry> {
    // SAFETY: the parent hands this descriptor over for this process alone and
    // nothing else in the daemon owns it.
    let mut pipe = unsafe { fs::File::from_raw_fd(fd) };
    let mut wire = String::new();
    pipe.read_to_string(&mut wire)
        .map_err(|_| io::Error::other("failed to read the secret handoff"))?;
    RegistryHandoff::from_wire(wire)
        .into_registry()
        .map_err(|err| io::Error::other(err.to_string()))
}

#[cfg(not(unix))]
fn read_secret_registry(_fd: i32) -> io::Result<SentinelRegistry> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the secret handoff needs inherited descriptors",
    ))
}

fn process_env(key: &str) -> Option<String> {
    SystemEnv.var(key).ok()
}

fn read_policy_file(path: &str) -> io::Result<String> {
    fs::read_to_string(path)
}

/// Spawns a background task that polls `policy_file`'s mtime every
/// `poll_interval` and, when it changes, re-reads and re-parses the file,
/// swapping it into `policy` on success. Parse/read failures are logged and
/// the last-known-good policy keeps serving; the observed mtime is only
/// updated on success so a broken file is retried every tick rather than
/// silently pinned to a bad state, at the cost of repeated warnings while it
/// stays broken. A policy that needs TLS termination for a domain outside
/// the CA's name constraints is rejected whole and the previous policy keeps
/// serving, since the CA can only be replaced by a restart.
fn spawn_policy_reload_task(
    policy_file: String,
    policy: PolicyHandle,
    poll_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_modified: Option<SystemTime> = None;
        let mut interval = tokio::time::interval(poll_interval);

        loop {
            interval.tick().await;

            let modified = match std::fs::metadata(&policy_file).and_then(|m| m.modified()) {
                Ok(modified) => modified,
                Err(err) => {
                    warn!(policy_file = %policy_file, error = %err, "failed to stat policy file for reload");
                    continue;
                }
            };

            if last_modified == Some(modified) {
                continue;
            }

            let contents = match std::fs::read_to_string(&policy_file) {
                Ok(contents) => contents,
                Err(err) => {
                    warn!(policy_file = %policy_file, error = %err, "failed to read policy file for reload");
                    continue;
                }
            };

            let new_policy: NetworkPolicy = match serde_json::from_str(&contents) {
                Ok(policy) => policy,
                Err(err) => {
                    warn!(policy_file = %policy_file, error = %err, "failed to parse policy file for reload");
                    continue;
                }
            };

            if let Err(PolicyRejectedError::OutsideCaConstraints { domains }) = policy.replace(new_policy).await {
                warn!(
                    policy_file = %policy_file,
                    domains = ?domains,
                    "rejected policy reload: domains need TLS interception outside the CA's name constraints; a CA rotation is required"
                );
                last_modified = Some(modified);
                continue;
            }

            last_modified = Some(modified);
            info!(policy_file = %policy_file, "reloaded network policy");
        }
    })
}

async fn run(config: Config) -> io::Result<()> {
    let listener = match &config.listen_addr {
        Some(addr) => {
            let listener = TcpListener::bind(addr).await?;
            info!(listen_addr = %addr, "sandbox-egress-proxyd listening");
            Some(listener)
        }
        None => None,
    };

    if matches!(config.upstream_exemption, UpstreamExemption::Uid(_)) {
        let actual = dial::effective_uid()?;
        config
            .upstream_exemption
            .verify_uid(actual)
            .map_err(|err| io::Error::other(format!("{EXEMPT_UID_ENV_VAR}: {err}")))?;
    }
    let marking = config
        .upstream_exemption
        .marking_for_transparent_listener(listener.is_some());
    marking.verify().map_err(|err| {
        io::Error::other(format!(
            "the transparent listener requires marking upstream sockets, which needs CAP_NET_ADMIN effective in this process (or {EXEMPT_UID_ENV_VAR}): {err}"
        ))
    })?;

    install_default_crypto_provider();
    let permitted_domains = permitted_domains_for(&config.policy)
        .map_err(|err| io::Error::other(format!("invalid domain rule for sandbox CA: {err}")))?;
    let ca = match &config.supplied_ca {
        Some((cert, key)) => SandboxCa::from_files(
            std::path::Path::new(cert),
            std::path::Path::new(key),
            permitted_domains.clone(),
        )
        .map(SandboxCa::permitting_every_name)
        .map_err(|err| io::Error::other(format!("failed to load sandbox CA: {err}")))?,
        None => SandboxCa::generate(permitted_domains.clone())
            .map_err(|err| io::Error::other(format!("failed to generate sandbox CA: {err}")))?,
    };

    if let Some(ca_out_file) = &config.ca_out_file {
        fs::write(ca_out_file, ca.root_cert_pem())?;
        info!(ca_out_file, "wrote sandbox CA root certificate");
    }

    let credential_source: Arc<dyn CredentialSource> = match &config.credential_broker {
        Some(broker) => {
            let providers = referenced_credential_providers(&config.policy);
            let connected = match &broker.fence {
                Some(fence) => {
                    BrokerCredentialSource::connect_fenced(
                        &broker.nats_url,
                        broker.tenant.clone(),
                        &providers,
                        fence.clone(),
                    )
                    .await
                }
                None => BrokerCredentialSource::connect(&broker.nats_url, broker.tenant.clone(), &providers).await,
            };
            let source =
                connected.map_err(|err| io::Error::other(format!("failed to connect to credential broker: {err}")))?;
            Arc::new(source)
        }
        None => Arc::new(UnconfiguredCredentialSource),
    };

    if config.oidc_identity.is_none() && policy_has_forward_rule(&config.policy) {
        return Err(io::Error::other(format!(
            "policy has a Forward domain rule but {OIDC_TENANT_ID_ENV_VAR}/{OIDC_LEASE_ID_ENV_VAR}/{OIDC_SUBJECT_ENV_VAR} are not set"
        )));
    }

    let oidc_identity = match &config.oidc_identity {
        Some(oidc) => {
            let signer = match &oidc.signing_key_pem_file {
                Some(pem_file) => {
                    let pem = fs::read_to_string(pem_file)?;
                    OidcSigner::from_ec_pkcs8_pem(&pem).map_err(|err| {
                        io::Error::other(format!(
                            "failed to load {OIDC_SIGNING_KEY_PEM_FILE_ENV_VAR}={pem_file}: {err}"
                        ))
                    })?
                }
                None => OidcSigner::generate()
                    .map_err(|err| io::Error::other(format!("failed to generate OIDC signing key: {err}")))?,
            };
            OidcIdentityContext {
                signer,
                issuer: OIDC_ISSUER.to_string(),
                subject: oidc.subject.clone(),
                tenant_id: oidc.tenant_id.clone(),
                lease_id: oidc.lease_id.clone(),
            }
        }
        // No Forward rule in the policy (checked above), so this identity is
        // never actually used to mint a token; built anyway so
        // `TlsInterceptContext` never needs an `Option` here (see its doc
        // comment on why `Forward` support is unconditional on the type).
        None => OidcIdentityContext::unconfigured(
            OidcSigner::generate()
                .map_err(|err| io::Error::other(format!("failed to generate OIDC signing key: {err}")))?,
            OIDC_ISSUER,
        )?,
    };

    let mut builder = ProxyServer::builder(config.policy, ca)
        .credential_source(credential_source)
        .oidc_identity(oidc_identity)
        .marking(marking)
        .network_options(config.network_options.clone());
    let policy_handle = builder.policy_handle();

    let decider = match &config.decider {
        Some(decider) => {
            let client = trogon_sandbox_protocol::nats_connect::connect(&decider.nats_url)
                .await
                .map_err(|err| io::Error::other(format!("failed to connect for decider: {err}")))?;
            let amender = LivePolicyAmender::new(
                decider.lease_id.clone(),
                policy_handle.shared(),
                Arc::new(NatsPolicyPublisher::new(client.clone())),
            );
            Some(Arc::new(Decider::new(
                decider.lease_id.clone(),
                Arc::new(NatsDeciderTransport::new(client)),
                Arc::new(amender),
                decider.limits,
            )))
        }
        None => None,
    };

    let violations = match &config.violation_events {
        Some(events) => {
            let client = trogon_sandbox_protocol::nats_connect::connect(&events.nats_url)
                .await
                .map_err(|err| io::Error::other(format!("failed to connect for violation events: {err}")))?;
            Some(Arc::new(ViolationReporter::new(
                events.lease_id.clone(),
                Arc::new(NatsViolationSink::new(client)),
                ViolationRateLimit::default(),
            )))
        }
        None => None,
    };

    if let Some(decider) = decider {
        builder = builder.decider(decider);
    }
    if let Some(fd) = config.secrets_fd {
        let registry = read_secret_registry(fd)?;
        info!(secrets = registry.len(), "loaded the secret handoff");
        let mut substitution = SecretSubstitution::new(Arc::new(registry));
        if let Some(violations) = &violations {
            substitution = substitution.with_observer(violations.clone());
        }
        builder = builder.secrets(substitution);
    }
    if let Some(violations) = violations {
        builder = builder.violations(violations);
    }

    spawn_policy_reload_task(config.policy_file.clone(), policy_handle, POLICY_RELOAD_POLL_INTERVAL);

    if let Some(listener) = listener {
        builder = builder.transparent(listener);
    }
    if let Some(mux) = &config.mux {
        let mux_listener = match mux.inherited_fd {
            #[cfg(unix)]
            Some(fd) => Listener::from_inherited_fd(fd)?,
            #[cfg(not(unix))]
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "inherited listening descriptors are not available on this platform",
                ));
            }
            None => Listener::bind(&mux.listen_addr).await?,
        };
        info!(listen_addr = %mux.listen_addr, "explicit proxy listening");
        builder = builder
            .explicit(mux_listener, mux.token.clone())
            .ingress_limits(IngressLimits::proxy_only());
        builder = if cfg!(windows) {
            builder.advertise_crl()
        } else {
            builder.serve_crl()
        };
    }

    builder.start()?.wait().await
}

/// Parses the flags and environment, runs the proxy until it stops, and
/// returns the process exit code.
async fn serve(
    args: impl IntoIterator<Item = String>,
    get_env: impl Fn(&str) -> Option<String>,
    read_file: impl Fn(&str) -> io::Result<String>,
) -> i32 {
    let config = match ListenFlags::parse(args)
        .and_then(|flags| load_config(&get_env, read_file).and_then(|config| flags.apply(config, &get_env)))
    {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(error = %err, "invalid configuration");
            return 1;
        }
    };

    let outcome = run(config).await;
    if let Err(err) = &outcome {
        tracing::error!(error = %err, "the proxy stopped");
    }
    i32::from(outcome.is_err())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    if let Err(err) = env_encoding::reject_non_unicode(std::env::vars_os()) {
        tracing::error!(error = %err, "invalid configuration");
        std::process::exit(1);
    }
    std::process::exit(serve(std::env::args().skip(1), process_env, read_policy_file).await);
}

#[cfg(test)]
mod tests;
