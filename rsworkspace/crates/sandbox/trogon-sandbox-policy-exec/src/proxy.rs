//! The egress proxy of one command, served from a task in this process.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::NetworkOptions;
use trogon_sandbox_contract::{ExternalProxy, LeaseId, NetworkPolicy, Port};
#[cfg(target_os = "linux")]
use trogon_sandbox_egress_proxy::ingress::ListenAddress;
use trogon_sandbox_egress_proxy::ingress::Listener;
use trogon_sandbox_egress_proxy::{
    AskCallback, CallbackRequestFilter, CommandId, CredentialPlacement, DeciderTransport, DenialReasons, HeaderMutator,
    ProxyServer, ProxyToken, RequestCallback, RequestDecider, RequestFilter, SandboxCa, SecretSubstitution,
    StoreViolationSink, ViolationFilter, ViolationRateLimit, ViolationReporter, ViolationStore, all_of,
    in_process_decider, install_default_crypto_provider, permitted_domains_for, workload_trust_bundle,
};
use trogon_sandbox_egress_proxy::{DeciderLimits, PolicyHandle, TunnelGate};
use trogon_sandbox_subprocess_worker::EgressPolicy;

use crate::constants::{CA_FILE_NAME, PROXY_SHUTDOWN_GRACE, SOCKS5_VERSION};
use crate::credentials::PreparedCredentials;
use crate::tls_intercept::requires_tls_intercept;
#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::isolation::relay::RelayEgress;
use trogon_sandbox_subprocess_worker::lease_proxy::LeaseProxyToken;
#[cfg(target_os = "linux")]
use trogon_sandbox_subprocess_worker::proxy_env::HostProxyPorts;
use trogon_sandbox_subprocess_worker::proxy_env::{CaCertificate, ProxyCredentials, ProxyEndpoint, ProxyEnvironment};
use trogon_sandbox_subprocess_worker::tool_proxy_env::ToolProxyEnv;

#[derive(Clone, Default)]
pub(crate) struct Callbacks {
    pub ask: Option<Arc<dyn AskCallback>>,
    pub request: Option<Arc<dyn RequestCallback>>,
    pub network_options: NetworkOptions,
    pub ask_limits: DeciderLimits,
    pub tools: ToolProxyEnv,
    pub plaintext_header_set: bool,
    pub header_mutator: Option<Arc<dyn HeaderMutator>>,
    pub plaintext_header_mutator: Option<Arc<dyn HeaderMutator>>,
    pub request_decider: Option<RequestDeciderSetup>,
    pub tunnel_gate: Option<Arc<TunnelGate>>,
}

#[derive(Clone)]
pub(crate) struct RequestDeciderSetup {
    pub transport: Arc<dyn DeciderTransport>,
    pub limits: DeciderLimits,
    pub placements: Vec<CredentialPlacement>,
}

pub(crate) struct CommandProxy {
    server: Option<ProxyServer>,
    token: Option<LeaseProxyToken>,
    bridge: Option<tokio::task::JoinHandle<()>>,
    ca_file: Option<PathBuf>,
    ca_certificate_pem: Option<String>,
    tools: ToolProxyEnv,
    #[cfg(target_os = "linux")]
    socket: PathBuf,
    #[cfg(target_os = "linux")]
    host_ports: HostProxyPorts,
    #[cfg(target_os = "linux")]
    host_forwarder: Option<tokio::task::JoinHandle<()>>,
    #[cfg(target_os = "macos")]
    addr: std::net::SocketAddr,
    _dir: tempfile::TempDir,
}

impl CommandProxy {
    pub(crate) async fn start(
        policy: &NetworkPolicy,
        command: &CommandId,
        callbacks: &Callbacks,
        store: &ViolationStore,
        reasons: &DenialReasons,
        filter: ViolationFilter,
        credentials: &PreparedCredentials,
    ) -> anyhow::Result<Self> {
        install_default_crypto_provider();
        let dir = tempfile::Builder::new()
            .prefix("policy-exec-")
            .tempdir()
            .context("creating the proxy directory")?;
        restrict_to_owner(dir.path())?;

        let external = callbacks.network_options.external_proxy;
        if let Some(ports) = external.and_then(both_ports) {
            if credentials.is_masking() {
                tracing::warn!(
                    "credential masking has no effect: the external proxy ports replace the runtime's own proxy"
                );
            }
            return Self::external(ports, callbacks, dir).await;
        }

        let permitted = permitted_domains_for(policy)?;
        let ca = match &callbacks.network_options.ca_files {
            Some(files) => SandboxCa::from_files(files.cert_path(), files.key_path(), permitted)?,
            None => SandboxCa::generate(permitted)?,
        };
        let token = match external {
            Some(external) if external.http_port().is_some() => None,
            _ => Some(LeaseProxyToken::generate()?),
        };
        let lease = LeaseId::new(command.to_string());
        let sink =
            Arc::new(StoreViolationSink::new(store.clone(), command.clone(), reasons.clone()).with_filter(filter));
        let reporter = Arc::new(ViolationReporter::new(
            lease.clone(),
            sink,
            ViolationRateLimit::default(),
        ));

        let intercepts_tls = requires_tls_intercept(policy)
            || credentials.is_masking()
            || callbacks.header_mutator.is_some()
            || callbacks.network_options.terminate_allowed_tls.is_enabled();
        let (listener, local_endpoint) = bind_listener(&dir).await?;
        let builder = ProxyServer::builder(policy.clone(), ca);
        let mut builder = match &token {
            Some(token) => builder.explicit(listener, ProxyToken::new(token.expose())?),
            None => builder.explicit_open(listener),
        }
        .violations(Arc::clone(&reporter))
        .denial_reasons(reasons.clone())
        .network_options(callbacks.network_options.clone())
        .decision_window_capture(!intercepts_tls);
        if let Some(gate) = &callbacks.tunnel_gate {
            builder = builder.tunnel_gate(Arc::clone(gate));
        }
        if let Some(ask) = &callbacks.ask {
            builder = builder.decider(Arc::new(in_process_decider(
                lease,
                Arc::clone(ask),
                reasons.clone(),
                callbacks.ask_limits,
            )));
        }
        if let Some((registry, sigv4)) = credentials.registry() {
            builder = builder.secrets(
                SecretSubstitution::new(registry)
                    .with_observer(reporter.clone())
                    .with_sigv4_policies(sigv4),
            );
            builder = builder.plaintext_inject(credentials.plaintext_inject());
        }
        builder = builder.plaintext_header_set(callbacks.plaintext_header_set);
        let mut request_filters: Vec<Arc<dyn RequestFilter>> = Vec::new();
        if let Some(request) = &callbacks.request {
            request_filters.push(Arc::new(CallbackRequestFilter::new(Arc::clone(request))));
        }
        if let Some(setup) = &callbacks.request_decider {
            request_filters.push(Arc::new(
                RequestDecider::new(
                    LeaseId::new(command.to_string()),
                    Arc::clone(&setup.transport),
                    setup.limits,
                )
                .credential_placements(setup.placements.clone()),
            ));
        }
        match request_filters.len() {
            0 => {}
            1 => builder = builder.request_filter(request_filters.remove(0)),
            _ => builder = builder.request_filter(all_of(request_filters)),
        }
        if let Some(mutator) = &callbacks.header_mutator {
            builder = builder.header_mutator(Arc::clone(mutator));
        }
        if let Some(mutator) = &callbacks.plaintext_header_mutator {
            builder = builder.plaintext_header_mutator(Arc::clone(mutator));
        }
        let server = builder.start().context("starting the egress proxy")?;
        let server_ca_pem = server.ca_certificate_pem().to_string();

        let ca_file = if intercepts_tls {
            let path = dir.path().join(CA_FILE_NAME);
            let bundle = workload_trust_bundle(
                server.ca_certificate_pem(),
                callbacks.network_options.extra_ca_cert_paths.as_slice(),
            );
            std::fs::write(&path, bundle).context("writing the workload trust bundle")?;
            Some(path)
        } else {
            None
        };

        #[cfg(target_os = "linux")]
        let (host_ports, host_forwarder) = host_ports_beside(external, &local_endpoint).await?;
        let (endpoint, bridge) = match external {
            Some(external) => {
                let targets = ExternalTargets::beside_local(external, local_target(&local_endpoint));
                let (endpoint, bridge) = dispatcher(targets, &dir).await?;
                (endpoint, Some(bridge))
            }
            None => (local_endpoint, None),
        };

        Ok(Self {
            server: Some(server),
            token,
            bridge,
            ca_file,
            ca_certificate_pem: Some(server_ca_pem),
            tools: callbacks.tools.clone(),
            #[cfg(target_os = "linux")]
            socket: endpoint,
            #[cfg(target_os = "linux")]
            host_ports,
            #[cfg(target_os = "linux")]
            host_forwarder: Some(host_forwarder),
            #[cfg(target_os = "macos")]
            addr: endpoint,
            _dir: dir,
        })
    }

    async fn external(
        (http, socks): (Port, Port),
        callbacks: &Callbacks,
        dir: tempfile::TempDir,
    ) -> anyhow::Result<Self> {
        let targets = ExternalTargets::between_externals(http, socks);
        #[cfg(target_os = "macos")]
        let (endpoint, bridge) = match targets.single_tcp() {
            Some(addr) => (addr, None),
            None => {
                let (endpoint, bridge) = dispatcher(targets, &dir).await?;
                (endpoint, Some(bridge))
            }
        };
        #[cfg(target_os = "linux")]
        let (endpoint, bridge) = {
            let (endpoint, bridge) = dispatcher(targets, &dir).await?;
            (endpoint, Some(bridge))
        };
        Ok(Self {
            server: None,
            token: None,
            bridge,
            ca_file: None,
            ca_certificate_pem: None,
            tools: callbacks.tools.clone(),
            #[cfg(target_os = "linux")]
            socket: endpoint,
            #[cfg(target_os = "linux")]
            host_ports: HostProxyPorts::new(http, socks),
            #[cfg(target_os = "linux")]
            host_forwarder: None,
            #[cfg(target_os = "macos")]
            addr: endpoint,
            _dir: dir,
        })
    }

    pub(crate) fn ca_certificate_pem(&self) -> Option<&str> {
        self.ca_certificate_pem.as_deref()
    }

    pub(crate) fn ca_file(&self) -> Option<&std::path::Path> {
        self.ca_file.as_deref()
    }

    pub(crate) fn policy_handle(&self) -> Option<PolicyHandle> {
        self.server.as_ref().map(ProxyServer::policy_handle)
    }

    pub(crate) fn environment(&self) -> ProxyEnvironment {
        #[cfg(target_os = "linux")]
        let endpoint = ProxyEndpoint::relay();
        #[cfg(target_os = "macos")]
        let endpoint = ProxyEndpoint::loopback(self.addr);
        let credentials = match &self.token {
            Some(token) => ProxyCredentials::new(token.clone()),
            None => ProxyCredentials::anonymous(),
        };
        let environment = ProxyEnvironment::new(endpoint, credentials, self.ca_file.clone().map(CaCertificate::new))
            .with_tools(self.tools.clone());
        #[cfg(target_os = "linux")]
        let environment = environment.with_host_ports(self.host_ports);
        environment
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn egress(&self, helper: Option<&std::path::Path>) -> anyhow::Result<EgressPolicy> {
        let relay = match helper {
            Some(binary) => RelayEgress {
                proxy_socket: self.socket.clone(),
                relay_binary: binary.to_path_buf(),
            },
            None => RelayEgress::with_current_binary(self.socket.clone())
                .context("locating the running executable to use as the relay")?,
        };
        Ok(EgressPolicy::Relay(relay))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn egress(&self, _helper: Option<&std::path::Path>) -> anyhow::Result<EgressPolicy> {
        Ok(EgressPolicy::Loopback(self.addr))
    }

    pub(crate) async fn shutdown(self) {
        if let Some(bridge) = &self.bridge {
            bridge.abort();
        }
        #[cfg(target_os = "linux")]
        if let Some(forwarder) = &self.host_forwarder {
            forwarder.abort();
        }
        if let Some(server) = self.server
            && let Err(err) = server.shutdown(PROXY_SHUTDOWN_GRACE).await
        {
            tracing::warn!(error = %err, "the egress proxy did not stop cleanly");
        }
    }
}

#[derive(Clone)]
enum Target {
    Tcp(std::net::SocketAddr),
    #[cfg(target_os = "linux")]
    Unix(PathBuf),
}

#[cfg(target_os = "linux")]
fn local_target(endpoint: &std::path::Path) -> Target {
    Target::Unix(endpoint.to_path_buf())
}

#[cfg(target_os = "macos")]
fn local_target(endpoint: &std::net::SocketAddr) -> Target {
    Target::Tcp(*endpoint)
}

#[derive(Clone)]
struct ExternalTargets {
    http: Target,
    socks: Target,
}

fn loopback(port: Port) -> Target {
    Target::Tcp(std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port.get())))
}

impl ExternalTargets {
    fn between_externals(http: Port, socks: Port) -> Self {
        Self {
            http: loopback(http),
            socks: loopback(socks),
        }
    }

    fn beside_local(external: ExternalProxy, local: Target) -> Self {
        let side = |port: Option<Port>| port.map_or_else(|| local.clone(), loopback);
        Self {
            http: side(external.http_port()),
            socks: side(external.socks_port()),
        }
    }

    #[cfg(target_os = "macos")]
    fn single_tcp(&self) -> Option<std::net::SocketAddr> {
        match (&self.http, &self.socks) {
            (Target::Tcp(http), Target::Tcp(socks)) if http == socks => Some(*http),
            _ => None,
        }
    }

    fn for_first_byte(&self, byte: u8) -> &Target {
        if byte == SOCKS5_VERSION {
            &self.socks
        } else {
            &self.http
        }
    }
}

async fn splice<I, O>(mut inbound: I, mut outbound: O, first: u8)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    O: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if outbound.write_all(&[first]).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
}

async fn forward<S>(mut inbound: S, targets: ExternalTargets)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut first = [0u8; 1];
    if inbound.read_exact(&mut first).await.is_err() {
        return;
    }
    match targets.for_first_byte(first[0]) {
        Target::Tcp(addr) => {
            if let Ok(outbound) = tokio::net::TcpStream::connect(addr).await {
                splice(inbound, outbound, first[0]).await;
            }
        }
        #[cfg(target_os = "linux")]
        Target::Unix(path) => {
            if let Ok(outbound) = tokio::net::UnixStream::connect(path).await {
                splice(inbound, outbound, first[0]).await;
            }
        }
    }
}

/// The two ports of an external proxy that serves both protocols, which makes
/// the command's own proxy unnecessary.
fn both_ports(external: ExternalProxy) -> Option<(Port, Port)> {
    Some((external.http_port()?, external.socks_port()?))
}

/// Whether the external proxy serves both protocols, so no proxy of the
/// command's own is started.
pub(crate) fn replaces_runtime_proxy(external: ExternalProxy) -> bool {
    both_ports(external).is_some()
}

fn restrict_to_owner(path: &std::path::Path) -> anyhow::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .context("restricting the proxy directory to its owner")
}

#[cfg(target_os = "linux")]
async fn dispatcher(
    targets: ExternalTargets,
    dir: &tempfile::TempDir,
) -> anyhow::Result<(PathBuf, tokio::task::JoinHandle<()>)> {
    let socket = dir.path().join("dispatch.sock");
    let listener =
        tokio::net::UnixListener::bind(&socket).context("binding the dispatcher for the external proxies")?;
    let task = tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            tokio::spawn(forward(inbound, targets.clone()));
        }
    });
    Ok((socket, task))
}

#[cfg(target_os = "macos")]
async fn dispatcher(
    targets: ExternalTargets,
    _dir: &tempfile::TempDir,
) -> anyhow::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding the dispatcher for the external proxies")?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            tokio::spawn(forward(inbound, targets.clone()));
        }
    });
    Ok((addr, task))
}

/// The host ports named to a Linux payload: an external proxy's own, and for
/// the side the runtime serves a loopback listener leading to its socket.
#[cfg(target_os = "linux")]
async fn host_ports_beside(
    external: Option<ExternalProxy>,
    local: &std::path::Path,
) -> anyhow::Result<(HostProxyPorts, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding the host proxy port")?;
    let port = Port::new(listener.local_addr()?.port())?;
    let targets = ExternalTargets {
        http: Target::Unix(local.to_path_buf()),
        socks: Target::Unix(local.to_path_buf()),
    };
    let task = tokio::spawn(async move {
        while let Ok((inbound, _)) = listener.accept().await {
            tokio::spawn(forward(inbound, targets.clone()));
        }
    });
    let side = |external_port: Option<Port>| external_port.unwrap_or(port);
    let ports = HostProxyPorts::new(
        side(external.and_then(|external| external.http_port())),
        side(external.and_then(|external| external.socks_port())),
    );
    Ok((ports, task))
}

#[cfg(target_os = "linux")]
async fn bind_listener(dir: &tempfile::TempDir) -> anyhow::Result<(Listener, PathBuf)> {
    let socket = dir.path().join("proxy.sock");
    let listener = Listener::bind(&ListenAddress::Unix(socket.clone()))
        .await
        .context("binding the proxy socket")?;
    Ok((listener, socket))
}

#[cfg(target_os = "macos")]
async fn bind_listener(_dir: &tempfile::TempDir) -> anyhow::Result<(Listener, std::net::SocketAddr)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding the loopback proxy listener")?;
    let addr = listener.local_addr()?;
    Ok((Listener::Tcp(listener), addr))
}

#[cfg(test)]
mod tests;
