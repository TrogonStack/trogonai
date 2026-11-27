//! What a payload needs to reach the world through its lease's egress proxy:
//! the proxy variables that point at the relay inside the sandbox, carrying
//! the lease token, and the trust variables for the CA that signs intercepted
//! connections.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context as _;
use rand_core::{OsRng, RngCore};
#[cfg(target_os = "linux")]
use trogon_sandbox_contract::Port;
use trogon_sandbox_egress_proxy::ingress::ProxyToken;
use trogon_sandbox_protocol::ca_trust_env;

#[cfg(unix)]
use crate::constants::NO_PROXY;
use crate::constants::{GUEST_CA_PATH, TOKEN_BYTES};
pub use crate::tool_proxy_env::ProxyHost;

use crate::attribution::{CommandAttribution, ProxyUser};
#[cfg(target_os = "linux")]
use crate::isolation::relay::{relay_listen_addr, relay_socks_listen_addr};
use crate::launch::ExtraBind;
#[cfg(unix)]
use crate::tool_proxy_env::{PREFER_IPV4_STACK, java_tool_options};
use crate::tool_proxy_env::{ProxyLogin, SshTransport, ToolProxyEnv};

/// Where the payload dials the proxy: the relay's loopback listener on Linux,
/// the proxy's own loopback listener on macOS and Windows. The variables spell
/// the host `localhost`. The Linux relays listen on two ports, one for HTTP
/// and one for SOCKS, and both reach the same proxy; elsewhere the one
/// listener serves both.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ProxyEndpoint {
    addr: SocketAddr,
    socks_addr: SocketAddr,
    host: ProxyHost,
}

impl ProxyEndpoint {
    #[cfg(target_os = "linux")]
    pub fn relay() -> Self {
        Self {
            addr: relay_listen_addr(),
            socks_addr: relay_socks_listen_addr(),
            host: ProxyHost::Localhost,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn relay_in_a_root_without_hosts() -> Self {
        Self {
            addr: relay_listen_addr(),
            socks_addr: relay_socks_listen_addr(),
            host: ProxyHost::Address,
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    pub fn loopback(addr: SocketAddr) -> Self {
        Self {
            addr,
            socks_addr: addr,
            host: ProxyHost::Localhost,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn socks_addr(&self) -> SocketAddr {
        self.socks_addr
    }

    pub fn host(&self) -> ProxyHost {
        self.host
    }
}

/// The host loopback ports the proxy is reached on, which a Linux payload is
/// told about for transparency: the relay inside the sandbox leads to them.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct HostProxyPorts {
    http: Port,
    socks: Port,
}

#[cfg(target_os = "linux")]
impl HostProxyPorts {
    pub fn new(http: Port, socks: Port) -> Self {
        Self { http, socks }
    }

    fn vars(&self) -> [(String, String); 2] {
        [
            ("SANDBOX_HOST_HTTP_PROXY_PORT".to_string(), self.http.to_string()),
            ("SANDBOX_HOST_SOCKS_PROXY_PORT".to_string(), self.socks.to_string()),
        ]
    }
}

/// The userinfo a client presents to the proxy. The token is hex, so it needs
/// no escaping in a URL, and it shows as `..` in `Debug`.
#[derive(Debug, Clone)]
pub struct ProxyCredentials {
    token: Option<LeaseProxyToken>,
    user: ProxyUser,
}

impl ProxyCredentials {
    pub fn new(token: LeaseProxyToken) -> Self {
        Self {
            token: Some(token),
            user: ProxyUser::bare(),
        }
    }

    /// For a proxy that takes no login, so the payload presents none.
    pub fn anonymous() -> Self {
        Self {
            token: None,
            user: ProxyUser::bare(),
        }
    }

    fn attributed(mut self, attribution: &CommandAttribution) -> Self {
        self.user = attribution.proxy_user();
        self
    }

    fn userinfo(&self) -> Option<String> {
        self.token
            .as_ref()
            .map(|token| format!("{}:{}", self.user.in_url(), token.expose()))
    }

    fn login(&self) -> Option<ProxyLogin<'_>> {
        self.token.as_ref().map(|token| ProxyLogin {
            user: self.user.clone(),
            token: token.expose(),
        })
    }
}

/// The root certificate the lease's proxy signs intercepted connections with,
/// as a file on the host.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CaCertificate {
    host_path: PathBuf,
}

impl CaCertificate {
    pub fn new(host_path: PathBuf) -> Self {
        Self { host_path }
    }

    pub fn guest_path() -> &'static str {
        GUEST_CA_PATH
    }

    #[cfg(target_os = "linux")]
    fn bind(&self) -> ExtraBind {
        ExtraBind {
            host_path: self.host_path.clone(),
            sandbox_path: GUEST_CA_PATH.to_string(),
            readonly: true,
            credential_mask: false,
        }
    }

    /// There is no mount namespace to place the file in, so the sandbox
    /// reads it where it is.
    #[cfg(any(target_os = "macos", windows))]
    fn bind(&self) -> ExtraBind {
        ExtraBind {
            host_path: self.host_path.clone(),
            sandbox_path: self.host_path.to_string_lossy().into_owned(),
            readonly: true,
            credential_mask: false,
        }
    }

    #[cfg(target_os = "linux")]
    fn trusted_path(&self) -> String {
        GUEST_CA_PATH.to_string()
    }

    /// Forward slashes on Windows, msys2 leaves `C:/..`
    /// alone and would mangle a backslash path in a command line.
    #[cfg(any(target_os = "macos", windows))]
    fn trusted_path(&self) -> String {
        let path = self.host_path.to_string_lossy();
        if cfg!(windows) {
            path.replace('\\', "/")
        } else {
            path.into_owned()
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProxyEnvironment {
    endpoint: ProxyEndpoint,
    credentials: ProxyCredentials,
    ca: Option<CaCertificate>,
    tools: ToolProxyEnv,
    #[cfg(target_os = "linux")]
    host_ports: Option<HostProxyPorts>,
}

impl ProxyEnvironment {
    pub fn new(endpoint: ProxyEndpoint, credentials: ProxyCredentials, ca: Option<CaCertificate>) -> Self {
        Self {
            endpoint,
            credentials,
            ca,
            tools: ToolProxyEnv::Disabled,
            #[cfg(target_os = "linux")]
            host_ports: None,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn with_host_ports(mut self, ports: HostProxyPorts) -> Self {
        self.host_ports = Some(ports);
        self
    }

    /// Names the command in the proxy user name, so the proxy can attribute
    /// what it denies to it.
    pub fn with_attribution(mut self, attribution: &CommandAttribution) -> Self {
        self.credentials = self.credentials.attributed(attribution);
        self
    }

    /// `JAVA_TOOL_OPTIONS` for a JVM in the payload: the proxy agent when
    /// there is one, `inherited` as the payload had it, and on macOS the IPv4
    /// stack flag that keeps the seatbelt loopback rule matching a JVM's
    /// connection to the proxy.
    #[cfg(unix)]
    pub fn java_tool_options(&self, inherited: Option<&str>) -> Option<String> {
        let flags: &[&str] = if cfg!(target_os = "macos") {
            &[PREFER_IPV4_STACK]
        } else {
            &[]
        };
        java_tool_options(self.tools.java_agent(), flags, inherited)
    }

    pub fn with_tools(mut self, tools: ToolProxyEnv) -> Self {
        self.tools = tools;
        self
    }

    pub fn ca_bind(&self) -> Option<ExtraBind> {
        self.ca.as_ref().map(CaCertificate::bind)
    }

    pub fn vars(&self) -> Vec<(String, String)> {
        let hostport = format!("{}:{}", self.endpoint.host(), self.endpoint.addr().port());
        let url = match self.credentials.userinfo() {
            Some(userinfo) => format!("http://{userinfo}@{hostport}"),
            None => format!("http://{hostport}"),
        };
        let mut vars = Vec::new();
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "GRPC_PROXY"] {
            vars.push((name.to_string(), url.clone()));
            vars.push((name.to_lowercase(), url.clone()));
        }
        #[cfg(unix)]
        let no_proxy = match self.tools {
            ToolProxyEnv::Enabled { .. } => NO_PROXY,
            ToolProxyEnv::Disabled => "",
        };
        #[cfg(not(unix))]
        let no_proxy = "";
        vars.push(("NO_PROXY".to_string(), no_proxy.to_string()));
        vars.push(("no_proxy".to_string(), no_proxy.to_string()));
        if let Some(ca) = &self.ca {
            vars.extend(
                ca_trust_env(&ca.trusted_path())
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value)),
            );
        }
        vars.extend(self.tools.vars(
            self.endpoint.host(),
            self.endpoint.addr(),
            self.endpoint.socks_addr(),
            self.credentials.login().as_ref(),
            SshTransport::native(),
        ));
        #[cfg(target_os = "linux")]
        vars.extend(self.host_ports.iter().flat_map(HostProxyPorts::vars));
        vars
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

#[cfg(all(test, target_os = "macos"))]
mod macos_tests;

/// The secret clients present to this lease's proxy. Never printed.
#[derive(Clone)]
pub struct LeaseProxyToken(String);

impl LeaseProxyToken {
    pub fn generate() -> anyhow::Result<Self> {
        let mut bytes = [0u8; TOKEN_BYTES];
        OsRng
            .try_fill_bytes(&mut bytes)
            .context("generating the lease proxy token")?;
        Self::parse(bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
    }

    /// Accepts only what a proxy URL can carry unescaped.
    pub fn parse(token: impl Into<String>) -> anyhow::Result<Self> {
        let token = token.into();
        ProxyToken::new(&token)?;
        anyhow::ensure!(
            token.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "the lease proxy token must be alphanumeric"
        );
        Ok(Self(token))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LeaseProxyToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LeaseProxyToken(..)")
    }
}
