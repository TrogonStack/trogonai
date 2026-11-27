//! The proxy-only mode: an HTTP proxy that serves clients outside any
//! sandbox and asks a decider on inherited streams about every request. The
//! decider's hello names the allowed and denied destinations; the listener
//! starts once it arrives.

use std::net::SocketAddr;
use std::os::fd::RawFd;

use crate::constants::{LISTEN_BACKLOG, PROXY_ONLY_SHUTDOWN_GRACE};
use tokio::io::{AsyncRead, AsyncWrite};
use trogon_sandbox_contract::NetworkOptions;
use trogon_sandbox_contract::{AllowedTlsTermination, DeniedDomain, DomainPattern, PatternTarget};
use trogon_sandbox_contract::{DomainRule, DomainRuleAction, IpRange, NetworkPolicy, OpaqueTunnelPolicy};
use trogon_sandbox_contract::{ResponseHeaderStrip, StrippedHeader};
use trogon_sandbox_egress_proxy::decider_stream::{DeciderOptions, FramedDecider};
use trogon_sandbox_egress_proxy::deny_header_style::DenyHeaderStyle;
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener};
use trogon_sandbox_egress_proxy::{
    BoundAddress, CredentialPlacement, IngressLimits, ProxyServer, SandboxCa, install_default_crypto_provider,
    permitted_domains_for,
};

pub type DeciderReader = Box<dyn AsyncRead + Unpin + Send>;
pub type DeciderWriter = Box<dyn AsyncWrite + Unpin + Send>;

/// Where the proxy accepts connections: a listening socket the parent bound
/// and passed down, or an address to bind.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ProxyOnlyListen {
    Fd(RawFd),
    Address { host: String, port: u16 },
}

pub struct ProxyOnlySpec {
    pub listen: ProxyOnlyListen,
    pub decider_in: DeciderReader,
    pub decider_out: DeciderWriter,
    pub ca_pem: Option<String>,
    pub strip_response_headers: Vec<String>,
    pub plaintext_header_set: bool,
}

pub struct ProxyOnly {
    server: Option<ProxyServer>,
    decider: FramedDecider,
    serving: String,
}

/// The CA PEM `--ca-fd` carries needs a private key and a certificate.
pub fn ca_pem_is_complete(pem: &str) -> bool {
    let key = pem.lines().any(|line| {
        let line = line.trim();
        line == "-----BEGIN PRIVATE KEY-----" || line == "-----BEGIN RSA PRIVATE KEY-----"
    });
    key && pem.contains("-----BEGIN CERTIFICATE-----")
}

impl ProxyOnly {
    /// Waits for the decider's hello, then starts listening. The error is the
    /// message to show the operator.
    pub async fn start(spec: ProxyOnlySpec) -> Result<Self, String> {
        install_default_crypto_provider();
        if let Some(pem) = &spec.ca_pem
            && !ca_pem_is_complete(pem)
        {
            return Err("proxy-only: the CA PEM needs a private key and a certificate".to_string());
        }
        let strip = spec
            .strip_response_headers
            .iter()
            .map(|name| StrippedHeader::new(name).map_err(|err| err.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let (hello, decider) = FramedDecider::open(
            spec.decider_in,
            spec.decider_out,
            DeciderOptions::default().credential_placements(vec![CredentialPlacement::github()]),
        )
        .await?;
        let started = Self::listen(
            &spec.listen,
            &hello,
            &spec.ca_pem,
            strip,
            spec.plaintext_header_set,
            &decider,
        )
        .await;
        match started {
            Ok((server, serving)) => Ok(Self {
                server: Some(server),
                decider,
                serving,
            }),
            Err(why) => {
                decider.close();
                Err(why)
            }
        }
    }

    async fn listen(
        listen: &ProxyOnlyListen,
        hello: &trogon_sandbox_egress_proxy::decider_stream::DeciderHello,
        ca_pem: &Option<String>,
        strip: Vec<StrippedHeader>,
        plaintext_header_set: bool,
        decider: &FramedDecider,
    ) -> Result<(ProxyServer, String), String> {
        let policy = policy_of(hello)?;
        let permitted = permitted_domains_for(&policy).map_err(|err| err.to_string())?;
        let ca = match ca_pem {
            Some(pem) => SandboxCa::from_pem_pair(pem, pem, permitted, "--ca-fd").map(SandboxCa::permitting_every_name),
            None => SandboxCa::generate(permitted),
        }
        .map_err(|err| err.to_string())?;
        let (listener, serving) = match listen {
            ProxyOnlyListen::Fd(fd) => (adopt_listener(*fd)?, format!("fd {fd}")),
            ProxyOnlyListen::Address { host, port } => {
                let address = resolve(host, *port).await?;
                let listener = Listener::bind(&ListenAddress::Tcp(address.to_string()))
                    .await
                    .map_err(|err| node_listen_error(&err, Some(address)))?;
                (listener, format!("{}:{}", address.ip(), address.port()))
            }
        };
        let options = NetworkOptions {
            strip_response_headers: ResponseHeaderStrip::new(strip),
            terminate_allowed_tls: AllowedTlsTermination::from(ca_pem.is_some()),
            ..NetworkOptions::default()
        };
        let server = ProxyServer::builder(policy, ca)
            .explicit_unauthenticated(listener)
            .ingress_limits(IngressLimits::proxy_only())
            .network_options(options)
            .plaintext_header_set(plaintext_header_set)
            .request_filter(std::sync::Arc::new(decider.clone()))
            .deny_header_style(DenyHeaderStyle::DenyReason)
            .start()
            .map_err(|err| err.to_string())?;
        let serving = match server.explicit_addr() {
            Some(BoundAddress::Tcp(bound)) if matches!(listen, ProxyOnlyListen::Address { .. }) => {
                format!("{}:{}", bound.ip(), bound.port())
            }
            _ => serving,
        };
        Ok((server, serving))
    }

    /// What the operator is told the proxy serves: `<address>:<port>` or `fd <N>`.
    pub fn serving(&self) -> &str {
        &self.serving
    }

    /// Resolves with the reason the decider's exchange ended.
    pub async fn decider_closed(&self) -> String {
        self.decider.closed().await
    }

    pub async fn shutdown(mut self) {
        self.decider.close();
        if let Some(server) = self.server.take() {
            let _ = server.shutdown(PROXY_ONLY_SHUTDOWN_GRACE).await;
        }
    }
}

async fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    if host.is_empty() {
        return Ok(SocketAddr::new(std::net::Ipv6Addr::UNSPECIFIED.into(), port));
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    tokio::net::lookup_host((host, port))
        .await
        .ok()
        .and_then(|mut found| found.next())
        .ok_or_else(|| format!("getaddrinfo ENOTFOUND {host}"))
}

/// Adopts the listening socket the way Node's `listen({ fd })` does: only a
/// stream socket qualifies, and it is put into the listening state here.
fn adopt_listener(fd: RawFd) -> Result<Listener, String> {
    let invalid = || "listen EINVAL: invalid argument".to_string();
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 || stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Err(invalid());
    }
    let mut kind: libc::c_int = 0;
    let mut length = size_of::<libc::c_int>() as libc::socklen_t;
    let status = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, (&raw mut kind).cast(), &mut length) };
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut name_length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let named = unsafe { libc::getsockname(fd, (&raw mut storage).cast(), &mut name_length) };
    let family = i32::from(storage.ss_family);
    let supported = status == 0
        && named == 0
        && kind == libc::SOCK_STREAM
        && matches!(family, libc::AF_UNIX | libc::AF_INET | libc::AF_INET6);
    if !supported {
        return Err(invalid());
    }
    if unsafe { libc::listen(fd, LISTEN_BACKLOG) } != 0 {
        return Err(node_listen_error(&std::io::Error::last_os_error(), None));
    }
    Listener::from_inherited_fd(fd).map_err(|err| node_listen_error(&err, None))
}

fn node_listen_error(error: &std::io::Error, address: Option<SocketAddr>) -> String {
    let (name, message) = match error.raw_os_error() {
        Some(libc::EADDRINUSE) => ("EADDRINUSE", "address already in use"),
        Some(libc::EADDRNOTAVAIL) => ("EADDRNOTAVAIL", "address not available"),
        Some(libc::EACCES) => ("EACCES", "permission denied"),
        Some(libc::EBADF) => ("EBADF", "bad file descriptor"),
        Some(libc::ENOTSOCK) => ("ENOTSOCK", "socket operation on non-socket"),
        Some(libc::EOPNOTSUPP) => ("EOPNOTSUPP", "operation not supported on socket"),
        _ => ("EINVAL", "invalid argument"),
    };
    match address {
        Some(address) => format!("listen {name}: {message} {}:{}", address.ip(), address.port()),
        None => format!("listen {name}: {message}"),
    }
}

fn policy_of(hello: &trogon_sandbox_egress_proxy::decider_stream::DeciderHello) -> Result<NetworkPolicy, String> {
    let allowed = hello
        .allowed_domains
        .iter()
        .map(|entry| DomainPattern::new(entry).map_err(|err| format!("{entry}: {err}")))
        .collect::<Result<Vec<_>, _>>()?;
    let denied = hello
        .denied_domains
        .iter()
        .map(|entry| DeniedDomain::new(entry).map_err(|err| format!("{entry}: {err}")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NetworkPolicy::UserDefined {
        allow_ranges: allowed
            .iter()
            .filter(|pattern| pattern.port().is_none())
            .filter_map(|pattern| match pattern.target() {
                PatternTarget::Exact(host) => host.as_ip(),
                PatternTarget::Any | PatternTarget::Subdomains(_) => None,
            })
            .map(|address| {
                let prefix = if address.is_ipv4() { 32 } else { 128 };
                IpRange::new(format!("{address}/{prefix}")).map_err(|err| err.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        rules: allowed
            .into_iter()
            .map(|domain| DomainRule {
                domain,
                action: DomainRuleAction::PassThrough,
            })
            .collect(),
        deny_ranges: Vec::new(),
        denied_domains: denied,
        opaque_tunnels: OpaqueTunnelPolicy::Refuse,
        request_rules: Vec::new(),
    })
}
