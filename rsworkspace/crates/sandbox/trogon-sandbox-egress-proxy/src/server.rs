//! Runs the egress proxy in process: accept loops over a transparent TCP
//! listener and an explicit proxy listener (TCP or unix socket), a live
//! policy that can be swapped while connections are served, and graceful
//! shutdown. Decider, violation reporting and credential sources are
//! injected, so nothing here needs NATS.

use std::io;
#[cfg(target_os = "linux")]
use std::mem;
#[cfg(target_os = "linux")]
use std::net::Ipv4Addr;
use std::net::SocketAddr;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, watch};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{error, info, warn};
use trogon_sandbox_contract::{DomainRuleAction, NetworkPolicy};
use trogon_sandbox_contract::{NetworkOptions, UpstreamCaCertificates};

use crate::credential_source::{CredentialSource, UnconfiguredCredentialSource};
use crate::decider::Decider;
use crate::deny_header_style::DenyHeaderStyle;
use crate::dial::UpstreamMarking;
use crate::header_mutator::HeaderMutator;
use crate::ingress::{AcceptedStream, Ingress, IngressLimits, IngressOutcome, Listener, PeerAddr, ProxyToken};
use crate::live_flows::LiveFlows;
use crate::oidc_signer::OidcSigner;
use crate::proxy::{OidcIdentityContext, ProxyOutcome, TlsInterceptContext, handle_connection};
use crate::request_filter::RequestFilter;
use crate::resolve::{AllowRanges, VettingResolver};
use crate::secret_substitution::SecretSubstitution;
use crate::tls_ca::{CrlUrl, DomainPattern, PermittedDomains, SandboxCa, SandboxCaError};
use crate::tunnel_gate::TunnelGate;
use crate::violation::ViolationReporter;
use crate::violation_store::DenialReasons;

mod constants;
pub use constants::OIDC_ISSUER;

/// Shared handle to the live [`NetworkPolicy`], swapped in place so accept-loop
/// readers always see the latest policy without any restart.
pub type SharedPolicy = Arc<RwLock<NetworkPolicy>>;

#[derive(Debug, thiserror::Error)]
pub enum PolicyRejectedError {
    #[error("domains need TLS interception outside the CA's name constraints, a CA rotation is required: {domains:?}")]
    OutsideCaConstraints { domains: Vec<String> },
}

/// The live policy together with the domains the lease's CA can vouch for.
/// A policy that needs TLS termination outside those domains is rejected
/// whole and the previous policy keeps serving, since the CA can only be
/// replaced by starting a new server.
#[derive(Clone)]
pub struct PolicyHandle {
    shared: SharedPolicy,
    permitted: PermittedDomains,
    flows: Arc<LiveFlows>,
    sweeper: Arc<std::sync::OnceLock<Arc<VettingResolver>>>,
}

impl PolicyHandle {
    pub fn new(policy: NetworkPolicy, permitted: PermittedDomains) -> Self {
        Self {
            shared: Arc::new(RwLock::new(policy)),
            permitted,
            flows: LiveFlows::new(),
            sweeper: Arc::default(),
        }
    }

    /// The flows the server registers when live-flow resweep is on.
    pub fn live_flows(&self) -> Arc<LiveFlows> {
        Arc::clone(&self.flows)
    }

    pub fn shared(&self) -> SharedPolicy {
        Arc::clone(&self.shared)
    }

    pub async fn current(&self) -> NetworkPolicy {
        self.shared.read().await.clone()
    }

    pub async fn replace(&self, policy: NetworkPolicy) -> Result<(), PolicyRejectedError> {
        let domains = intercept_domains_outside_ca(&policy, &self.permitted);
        if !domains.is_empty() {
            return Err(PolicyRejectedError::OutsideCaConstraints { domains });
        }
        *self.shared.write().await = policy.clone();
        if let Some(resolver) = self.sweeper.get() {
            let ended = self.flows.sweep(&policy, resolver).await;
            if ended > 0 {
                info!(ended, "policy replacement ended live flows it no longer admits");
            }
        }
        Ok(())
    }
}

/// The CA domain constraints a lease's policy implies.
pub fn permitted_domains_for(policy: &NetworkPolicy) -> Result<PermittedDomains, SandboxCaError> {
    match policy {
        NetworkPolicy::UserDefined { rules, .. } => PermittedDomains::from_rules(rules),
        NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => PermittedDomains::from_rules(&[]),
    }
}

/// Domains in `policy` that need TLS termination (`Transform` or `Forward`)
/// but fall outside what `permitted` lets the lease's CA vouch for.
pub fn intercept_domains_outside_ca(policy: &NetworkPolicy, permitted: &PermittedDomains) -> Vec<String> {
    let NetworkPolicy::UserDefined { rules, .. } = policy else {
        return Vec::new();
    };
    rules
        .iter()
        .filter(|rule| !matches!(rule.action, DomainRuleAction::PassThrough))
        .filter(|rule| DomainPattern::from_rule(&rule.domain).is_some_and(|pattern| !permitted.covers(&pattern)))
        .map(|rule| rule.domain.to_string())
        .collect()
}

/// Builds the `rustls::ClientConfig` used to re-encrypt to the real upstream
/// on a `Transform`/`Forward` domain rule: validated against the process's
/// system root store, not against the sandbox CA (which only signs the leaf
/// shown to the sandboxed client).
pub fn system_upstream_tls_config() -> Arc<rustls::ClientConfig> {
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(system_root_store())
            .with_no_client_auth(),
    )
}

fn system_root_store() -> rustls::RootCertStore {
    let mut root_store = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for err in &native.errors {
        warn!(error = %err, "failed to load a native root certificate");
    }
    for cert in native.certs {
        let _ = root_store.add(cert);
    }
    root_store
}

/// [`system_upstream_tls_config`] that also trusts `extra`. A bundle that
/// holds no certificate, or one that does not parse, is an error rather than
/// a silently smaller trust set.
pub fn upstream_tls_config_trusting(extra: &UpstreamCaCertificates) -> io::Result<Arc<rustls::ClientConfig>> {
    let mut root_store = system_root_store();
    for bundle in extra.iter() {
        let mut found = 0usize;
        for cert in CertificateDer::pem_slice_iter(bundle.as_bytes()) {
            let cert = cert.map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("upstream CA certificate does not parse: {err}"),
                )
            })?;
            root_store.add(cert).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("upstream CA certificate is not usable: {err}"),
                )
            })?;
            found += 1;
        }
        if found == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "upstream CA bundle holds no certificate",
            ));
        }
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundAddress {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl BoundAddress {
    fn of(listener: &Listener) -> io::Result<Self> {
        match listener {
            Listener::Tcp(listener) => listener.local_addr().map(Self::Tcp),
            #[cfg(unix)]
            Listener::Unix(listener) => {
                let addr = listener.local_addr()?;
                addr.as_pathname()
                    .map(|path| Self::Unix(path.to_path_buf()))
                    .ok_or_else(|| io::Error::other("unix listener has no path"))
            }
        }
    }
}

struct ExplicitListener {
    listener: Listener,
    token: Option<ProxyToken>,
    open_socks: bool,
    limits: IngressLimits,
}

pub struct ProxyServerBuilder {
    policy: PolicyHandle,
    ca: SandboxCa,
    transparent: Option<TcpListener>,
    explicit: Option<ExplicitListener>,
    upstream_tls_config: Option<Arc<rustls::ClientConfig>>,
    credential_source: Option<Arc<dyn CredentialSource>>,
    oidc_identity: Option<OidcIdentityContext>,
    marking: Option<UpstreamMarking>,
    secrets: Option<SecretSubstitution>,
    decider: Option<Arc<Decider>>,
    violations: Option<Arc<ViolationReporter>>,
    request_filter: Option<Arc<dyn RequestFilter>>,
    plaintext_header_set: bool,
    decision_window_capture: bool,
    plaintext_inject: bool,
    header_mutator: Option<Arc<dyn HeaderMutator>>,
    plaintext_header_mutator: Option<Arc<dyn HeaderMutator>>,
    resolver: Option<Arc<VettingResolver>>,
    network_options: NetworkOptions,
    tunnel_gate: Option<Arc<TunnelGate>>,
    denial_reasons: Option<DenialReasons>,
    deny_header_style: DenyHeaderStyle,
    serve_crl: bool,
    advertise_crl: bool,
}

impl ProxyServerBuilder {
    /// Handle to the live policy, available before [`Self::start`] so a
    /// decider can amend the policy the server is serving.
    pub fn policy_handle(&self) -> PolicyHandle {
        self.policy.clone()
    }

    /// Serves connections redirected to `listener` by a transparent redirect,
    /// recovering the original destination from the socket (Linux only).
    pub fn transparent(mut self, listener: TcpListener) -> Self {
        self.transparent = Some(listener);
        self
    }

    /// Serves HTTP CONNECT and SOCKS5 clients that present `token`.
    pub fn explicit(mut self, listener: Listener, token: ProxyToken) -> Self {
        self.explicit = Some(ExplicitListener {
            listener,
            token: Some(token),
            open_socks: false,
            limits: IngressLimits::default(),
        });
        self
    }

    /// Serves HTTP CONNECT and plain proxy clients without asking them to
    /// authenticate. SOCKS is not served.
    pub fn explicit_unauthenticated(mut self, listener: Listener) -> Self {
        self.explicit = Some(ExplicitListener {
            listener,
            token: None,
            open_socks: false,
            limits: IngressLimits::default(),
        });
        self
    }

    /// Serves HTTP CONNECT, plain proxy and SOCKS5 clients without asking them
    /// to authenticate.
    pub fn explicit_open(mut self, listener: Listener) -> Self {
        self.explicit = Some(ExplicitListener {
            listener,
            token: None,
            open_socks: true,
            limits: IngressLimits::default(),
        });
        self
    }

    /// Answers a plain `GET /ca.crl` on the explicit listener with an empty
    /// revocation list signed by the CA, ahead of any credential check.
    pub fn serve_crl(mut self) -> Self {
        self.serve_crl = true;
        self
    }

    /// Also makes every leaf name the loopback address of a TCP explicit
    /// listener as its CRL distribution point, so a client that checks
    /// revocation (Schannel, or OpenSSL with `CRL_CHECK`) finds the list
    /// instead of failing. For a client that reaches the listener on that
    /// address. Implies [`Self::serve_crl`].
    pub fn advertise_crl(mut self) -> Self {
        self.serve_crl = true;
        self.advertise_crl = true;
        self
    }

    pub fn ingress_limits(mut self, limits: IngressLimits) -> Self {
        if let Some(explicit) = &mut self.explicit {
            explicit.limits = limits;
        }
        self
    }

    pub fn upstream_tls_config(mut self, config: Arc<rustls::ClientConfig>) -> Self {
        self.upstream_tls_config = Some(config);
        self
    }

    pub fn credential_source(mut self, source: Arc<dyn CredentialSource>) -> Self {
        self.credential_source = Some(source);
        self
    }

    pub fn oidc_identity(mut self, identity: OidcIdentityContext) -> Self {
        self.oidc_identity = Some(identity);
        self
    }

    pub fn marking(mut self, marking: UpstreamMarking) -> Self {
        self.marking = Some(marking);
        self
    }

    pub fn secrets(mut self, secrets: SecretSubstitution) -> Self {
        self.secrets = Some(secrets);
        self
    }

    pub fn decider(mut self, decider: Arc<Decider>) -> Self {
        self.decider = Some(decider);
        self
    }

    pub fn violations(mut self, violations: Arc<ViolationReporter>) -> Self {
        self.violations = Some(violations);
        self
    }

    /// A filter consulted for each HTTP request the proxy can read, in
    /// addition to the policy's own request rules. Requests it or the rules
    /// deny are reported through [`Self::violations`].
    pub fn request_filter(mut self, filter: Arc<dyn RequestFilter>) -> Self {
        self.request_filter = Some(filter);
        self
    }

    /// Edits the headers of each terminated request just before it goes
    /// upstream.
    pub fn header_mutator(mut self, mutator: Arc<dyn HeaderMutator>) -> Self {
        self.header_mutator = Some(mutator);
        self
    }

    /// Edits the headers of each plain-HTTP request just before it goes
    /// upstream.
    pub fn plaintext_header_mutator(mut self, mutator: Arc<dyn HeaderMutator>) -> Self {
        self.plaintext_header_mutator = Some(mutator);
        self
    }

    pub fn decision_window_capture(mut self, enabled: bool) -> Self {
        self.decision_window_capture = enabled;
        self
    }

    pub fn plaintext_header_set(mut self, enabled: bool) -> Self {
        self.plaintext_header_set = enabled;
        self
    }

    pub fn plaintext_inject(mut self, enabled: bool) -> Self {
        self.plaintext_inject = enabled;
        self
    }

    pub fn resolver(mut self, resolver: Arc<VettingResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Optional network behaviour, every part of it off unless set. Extra
    /// upstream CAs do not combine with [`Self::upstream_tls_config`], and
    /// loopback names do not combine with [`Self::resolver`]; either pairing
    /// fails [`Self::start`].
    pub fn network_options(mut self, options: NetworkOptions) -> Self {
        self.network_options = options;
        self
    }

    /// The terminated-tunnel count, cap and handshake timeout the explicit
    /// proxy runs under. Shared with other proxies holding the same gate, and
    /// changeable while they serve; without one the proxy gets its own from
    /// the network options.
    pub fn tunnel_gate(mut self, gate: Arc<TunnelGate>) -> Self {
        self.tunnel_gate = Some(gate);
        self
    }

    /// Where the words behind a refusal are kept for the violation line.
    pub fn denial_reasons(mut self, reasons: DenialReasons) -> Self {
        self.denial_reasons = Some(reasons);
        self
    }

    /// How this server names the reason of a refusal it generates. Each
    /// server keeps its own style, so servers of different styles can share
    /// a process.
    pub fn deny_header_style(mut self, style: DenyHeaderStyle) -> Self {
        self.deny_header_style = style;
        self
    }

    pub fn start(self) -> io::Result<ProxyServer> {
        if self.transparent.is_none() && self.explicit.is_none() {
            return Err(io::Error::other("no listener configured"));
        }

        let marking = self
            .marking
            .unwrap_or_else(|| UpstreamMarking::for_transparent_listener(self.transparent.is_some()));
        marking.verify().map_err(|err| {
            io::Error::other(format!(
                "the transparent listener requires marking upstream sockets: {err}"
            ))
        })?;

        let options = Arc::new(self.network_options);
        let upstream_tls_config = match (self.upstream_tls_config, options.upstream_ca.is_empty()) {
            (Some(_), false) => {
                return Err(io::Error::other(
                    "extra upstream CA certificates cannot be combined with a custom upstream TLS config",
                ));
            }
            (Some(config), true) => config,
            (None, true) => system_upstream_tls_config(),
            (None, false) => upstream_tls_config_trusting(&options.upstream_ca)?,
        };
        let resolver = match self.resolver {
            Some(_)
                if options.loopback_names.is_enabled()
                    || options.private_ranges_by_name.is_enabled()
                    || !options.denied_resolved_addresses.is_empty() =>
            {
                return Err(io::Error::other(
                    "loopback names, private ranges by name and denied resolved addresses cannot be combined with a custom resolver",
                ));
            }
            Some(resolver) => resolver,
            None => {
                let mut resolver = VettingResolver::system();
                if let Some(ttl) = options.binding_ttl {
                    resolver = resolver.with_binding_ttl(ttl);
                }
                if options.loopback_names.is_enabled() {
                    resolver = resolver.with_loopback_names();
                }
                if options.private_ranges_by_name.is_enabled() {
                    resolver = resolver.with_private_ranges_by_name();
                }
                if !options.denied_resolved_addresses.is_empty() {
                    resolver =
                        resolver.with_listed_addresses(AllowRanges::from_ranges(&options.denied_resolved_addresses));
                }
                Arc::new(resolver)
            }
        };

        if options.resweep_live_flows.is_enabled() {
            let _ = self.policy.sweeper.set(Arc::clone(&resolver));
        }

        let ca_certificate_pem = self.ca.root_cert_pem();
        let crl = self.serve_crl.then(|| Arc::<[u8]>::from(self.ca.crl_der()));
        if self.advertise_crl
            && let Some(ExplicitListener { listener, .. }) = &self.explicit
            && let BoundAddress::Tcp(addr) = BoundAddress::of(listener)?
        {
            self.ca.set_crl_url(CrlUrl::loopback(addr.port()));
        }
        let oidc_identity = match self.oidc_identity {
            Some(identity) => identity,
            None => OidcIdentityContext::unconfigured(
                OidcSigner::generate()
                    .map_err(|err| io::Error::other(format!("failed to generate OIDC signing key: {err}")))?,
                OIDC_ISSUER,
            )?,
        };
        let mut tls_intercept = TlsInterceptContext::new(
            self.ca,
            upstream_tls_config,
            self.credential_source
                .unwrap_or_else(|| Arc::new(UnconfiguredCredentialSource)),
            oidc_identity,
        )
        .with_marking(marking)
        .with_network_options(Arc::clone(&options));
        if let Some(reasons) = self.denial_reasons {
            tls_intercept = tls_intercept.with_denial_reasons(reasons);
        }
        if let Some(decider) = &self.decider {
            tls_intercept = tls_intercept.with_decider(Arc::clone(decider));
        }
        if options.resweep_live_flows.is_enabled() {
            tls_intercept = tls_intercept.with_live_flows(self.policy.live_flows());
        }
        if let Some(secrets) = self.secrets {
            tls_intercept = tls_intercept.with_secrets(secrets);
        }
        if let Some(filter) = self.request_filter {
            tls_intercept = tls_intercept.with_request_filter(filter);
        }
        tls_intercept = tls_intercept
            .with_plaintext_header_set(self.plaintext_header_set)
            .with_decision_window_capture(self.decision_window_capture)
            .with_plaintext_inject(self.plaintext_inject);
        if let Some(mutator) = self.header_mutator {
            tls_intercept = tls_intercept.with_header_mutator(mutator);
        }
        if let Some(mutator) = self.plaintext_header_mutator {
            tls_intercept = tls_intercept.with_plaintext_header_mutator(mutator);
        }
        if let Some(violations) = &self.violations {
            tls_intercept = tls_intercept.with_request_denials(Arc::clone(violations) as _);
        }
        let tls_intercept = Arc::new(tls_intercept);

        let (shutdown, shutdown_signal) = watch::channel(None);
        let mut tasks = Vec::new();
        let mut transparent_addr = None;
        let mut explicit_addr = None;

        if let Some(listener) = self.transparent {
            transparent_addr = Some(listener.local_addr()?);
            tasks.push(tokio::spawn(serve_transparent(
                listener,
                self.policy.shared(),
                Arc::clone(&resolver),
                Arc::clone(&tls_intercept),
                self.violations.clone(),
                self.deny_header_style,
                shutdown_signal.clone(),
            )));
        }

        if let Some(interval) = options.revalidate_interval {
            tasks.push(tokio::spawn(revalidate_periodically(
                self.policy.clone(),
                Arc::clone(&resolver),
                interval,
                shutdown_signal.clone(),
            )));
        }

        if let Some(explicit) = self.explicit {
            explicit_addr = Some(BoundAddress::of(&explicit.listener)?);
            let mut ingress = match explicit.token {
                Some(token) => Ingress::new(token, resolver, explicit.limits),
                None if explicit.open_socks => Ingress::open(resolver, explicit.limits),
                None => Ingress::unauthenticated(resolver, explicit.limits),
            }
            .with_network_options(options);
            if let Some(crl) = crl {
                ingress = ingress.with_crl(crl);
            }
            if let Some(gate) = &self.tunnel_gate {
                ingress = ingress.with_tunnel_gate(Arc::clone(gate));
            }
            if let Some(decider) = &self.decider {
                ingress = ingress.with_decider(Arc::clone(decider));
            }
            tasks.push(tokio::spawn(serve_explicit(
                explicit.listener,
                self.policy.shared(),
                Arc::new(ingress),
                tls_intercept,
                self.violations,
                self.deny_header_style,
                shutdown_signal,
            )));
        }

        Ok(ProxyServer {
            policy: self.policy,
            ca_certificate_pem,
            transparent_addr,
            explicit_addr,
            shutdown,
            tasks,
        })
    }
}

pub struct ProxyServer {
    policy: PolicyHandle,
    ca_certificate_pem: String,
    transparent_addr: Option<SocketAddr>,
    explicit_addr: Option<BoundAddress>,
    shutdown: watch::Sender<Option<Duration>>,
    tasks: Vec<JoinHandle<io::Result<()>>>,
}

impl ProxyServer {
    pub fn builder(policy: NetworkPolicy, ca: SandboxCa) -> ProxyServerBuilder {
        ProxyServerBuilder {
            policy: PolicyHandle::new(policy, ca.permitted_domains().clone()),
            ca,
            transparent: None,
            explicit: None,
            upstream_tls_config: None,
            credential_source: None,
            oidc_identity: None,
            marking: None,
            secrets: None,
            decider: None,
            violations: None,
            request_filter: None,
            plaintext_header_set: false,
            decision_window_capture: false,
            plaintext_inject: false,
            header_mutator: None,
            plaintext_header_mutator: None,
            resolver: None,
            network_options: NetworkOptions::default(),
            tunnel_gate: None,
            denial_reasons: None,
            deny_header_style: DenyHeaderStyle::ProxyError,
            serve_crl: false,
            advertise_crl: false,
        }
    }

    pub fn policy_handle(&self) -> PolicyHandle {
        self.policy.clone()
    }

    /// Swaps the policy served to new connections. A rejected policy leaves
    /// the previous one in place.
    pub async fn replace_policy(&self, policy: NetworkPolicy) -> Result<(), PolicyRejectedError> {
        self.policy.replace(policy).await
    }

    pub fn transparent_addr(&self) -> Option<SocketAddr> {
        self.transparent_addr
    }

    pub fn explicit_addr(&self) -> Option<&BoundAddress> {
        self.explicit_addr.as_ref()
    }

    /// The root certificate sandboxed clients must trust for TLS termination.
    pub fn ca_certificate_pem(&self) -> &str {
        &self.ca_certificate_pem
    }

    /// Runs until the primary listener (the transparent one when present,
    /// otherwise the explicit one) fails to accept.
    pub async fn wait(mut self) -> io::Result<()> {
        let primary = self.tasks.remove(0);
        primary.await.map_err(io::Error::other)?
    }

    /// Stops accepting, lets in-flight connections finish for up to `grace`,
    /// then drops the rest.
    pub async fn shutdown(mut self, grace: Duration) -> io::Result<()> {
        let _ = self.shutdown.send(Some(grace));
        let mut outcome = Ok(());
        for task in std::mem::take(&mut self.tasks) {
            if let Err(err) = task.await.map_err(io::Error::other).and_then(|r| r) {
                outcome = Err(err);
            }
        }
        outcome
    }
}

impl Drop for ProxyServer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn drain(mut connections: JoinSet<()>, grace: Duration) {
    let _ = tokio::time::timeout(grace, async { while connections.join_next().await.is_some() {} }).await;
}

/// Re-asks every live flow, on a timer, whether the proxy's own resolution
/// still binds its name to its address, so an upstream that moved away from
/// an address ends the flows still using it. Stops on shutdown.
pub(crate) async fn revalidate_periodically(
    policy: PolicyHandle,
    resolver: Arc<VettingResolver>,
    interval: Duration,
    mut shutdown: watch::Receiver<Option<Duration>>,
) -> io::Result<()> {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => return Ok(()),
            _ = ticker.tick() => {
                let current = policy.current().await;
                let ended = policy.live_flows().sweep(&current, &resolver).await;
                if ended > 0 {
                    info!(ended, "periodic revalidation ended live flows whose name no longer resolves to their address");
                }
            }
        }
    }
}

fn requested_grace(signal: &watch::Receiver<Option<Duration>>) -> Duration {
    signal.borrow().unwrap_or_default()
}

async fn serve_transparent(
    listener: TcpListener,
    shared: SharedPolicy,
    resolver: Arc<VettingResolver>,
    tls_intercept: Arc<TlsInterceptContext>,
    violations: Option<Arc<ViolationReporter>>,
    style: DenyHeaderStyle,
    mut shutdown: watch::Receiver<Option<Duration>>,
) -> io::Result<()> {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                connections.spawn(style.scope(handle_accepted_connection(
                    stream,
                    peer_addr,
                    shared.clone(),
                    Arc::clone(&resolver),
                    Arc::clone(&tls_intercept),
                    violations.clone(),
                )));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drain(connections, requested_grace(&shutdown)).await;
    Ok(())
}

async fn serve_explicit(
    listener: Listener,
    shared: SharedPolicy,
    ingress: Arc<Ingress>,
    tls_intercept: Arc<TlsInterceptContext>,
    violations: Option<Arc<ViolationReporter>>,
    style: DenyHeaderStyle,
    mut shutdown: watch::Receiver<Option<Duration>>,
) -> io::Result<()> {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                connections.spawn(style.scope(handle_mux_connection(
                    stream,
                    peer_addr,
                    shared.clone(),
                    Arc::clone(&ingress),
                    Arc::clone(&tls_intercept),
                    violations.clone(),
                )));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    drain(connections, requested_grace(&shutdown)).await;
    Ok(())
}

#[cfg(target_os = "linux")]
fn recover_original_destination(stream: &TcpStream) -> io::Result<SocketAddr> {
    const SO_ORIGINAL_DST: libc::c_int = 80;

    let fd = stream.as_raw_fd();
    let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
    let mut len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;

    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_IP,
            SO_ORIGINAL_DST,
            &mut addr as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };

    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
    let port = u16::from_be(addr.sin_port);
    Ok(SocketAddr::from((ip, port)))
}

#[cfg(not(target_os = "linux"))]
fn recover_original_destination(_stream: &TcpStream) -> io::Result<SocketAddr> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "SO_ORIGINAL_DST recovery is only supported on Linux",
    ))
}

/// Handles a single accepted connection: recovers the original destination,
/// reads out (clones) the currently active policy just long enough to decide,
/// and hands off to [`handle_connection`]. The read guard on `shared` is held
/// only for the duration of the clone, never across the connection's splice,
/// so a concurrent policy swap is never blocked behind slow connections.
async fn handle_accepted_connection(
    stream: TcpStream,
    peer_addr: SocketAddr,
    shared: SharedPolicy,
    resolver: Arc<VettingResolver>,
    tls_intercept: Arc<TlsInterceptContext>,
    violations: Option<Arc<ViolationReporter>>,
) {
    match recover_original_destination(&stream) {
        Ok(dest) => handle_redirected(stream, peer_addr, dest, shared, resolver, tls_intercept, violations).await,
        Err(err) => error!(%peer_addr, error = %err, "failed to recover original destination"),
    }
}

async fn handle_redirected(
    stream: TcpStream,
    peer_addr: SocketAddr,
    dest: SocketAddr,
    shared: SharedPolicy,
    resolver: Arc<VettingResolver>,
    tls_intercept: Arc<TlsInterceptContext>,
    violations: Option<Arc<ViolationReporter>>,
) {
    let policy = shared.read().await.clone();

    let outcome = handle_connection(stream, &policy, dest.ip(), dest.port(), &resolver, &tls_intercept).await;

    if let Some(violations) = &violations {
        violations.report_outcome(&outcome, dest).await;
    }

    log_outcome(peer_addr, dest, &outcome);
}

fn log_outcome(peer_addr: SocketAddr, dest: SocketAddr, outcome: &ProxyOutcome) {
    match outcome {
        ProxyOutcome::Completed {
            bytes_client_to_upstream,
            bytes_upstream_to_client,
        } => {
            info!(
                %peer_addr,
                %dest,
                bytes_client_to_upstream,
                bytes_upstream_to_client,
                "connection completed"
            );
        }
        ProxyOutcome::Denied(_) => {
            warn!(%peer_addr, %dest, "connection denied by policy");
        }
        ProxyOutcome::UpstreamConnectFailed(err) => {
            error!(%peer_addr, %dest, error = %err, "upstream connect failed");
        }
        ProxyOutcome::SniffFailed(err) => {
            error!(%peer_addr, %dest, error = %err, "sniff failed");
        }
        ProxyOutcome::PostgresUpstreamConnectFailed(err) => {
            error!(%peer_addr, %dest, error = %err, "postgres upstream connect failed");
        }
        ProxyOutcome::PostgresNegotiationFailed(err) => {
            error!(%peer_addr, %dest, error = %err, "postgres negotiation failed");
        }
        ProxyOutcome::TlsInterceptRequiresSni => {
            error!(%peer_addr, %dest, "transform/forward rule matched with no SNI available");
        }
        ProxyOutcome::TlsInterceptFailed(err) => {
            error!(%peer_addr, %dest, error = %err, "tls intercept failed");
        }
    }
}

async fn handle_mux_connection(
    stream: AcceptedStream,
    peer_addr: PeerAddr,
    shared: SharedPolicy,
    ingress: Arc<Ingress>,
    tls_intercept: Arc<TlsInterceptContext>,
    violations: Option<Arc<ViolationReporter>>,
) {
    let policy = shared.read().await.clone();
    let outcome = ingress.serve(stream, &policy, &tls_intercept).await;

    if let Some(violations) = &violations {
        violations.report_ingress(&outcome).await;
    }

    match outcome {
        IngressOutcome::Proxied { target, outcome, .. } => match outcome {
            ProxyOutcome::Completed {
                bytes_client_to_upstream,
                bytes_upstream_to_client,
            } => info!(
                %peer_addr,
                %target,
                bytes_client_to_upstream,
                bytes_upstream_to_client,
                "proxied connection completed"
            ),
            ProxyOutcome::Denied(_) => warn!(%peer_addr, %target, "connection denied by policy"),
            other => error!(%peer_addr, %target, outcome = ?other, "proxied connection failed"),
        },
        IngressOutcome::Rejected(rejection) => {
            warn!(%peer_addr, reason = %rejection, "explicit proxy connection rejected");
        }
        IngressOutcome::CrlServed => info!(%peer_addr, "served the revocation list"),
    }
}

#[cfg(test)]
mod tests;
