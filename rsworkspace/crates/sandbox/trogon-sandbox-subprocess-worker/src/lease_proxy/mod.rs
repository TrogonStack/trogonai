//! One `trogon-sandbox-egress-proxyd` per lease, holding that lease's policy and
//! token and reachable only through a unix socket in a private directory.
//!
//! The proxy is a child process, not a task in the worker. It runs the same
//! daemon the other providers run, so the CA, policy reload, credential
//! injection and violation reporting are not reimplemented here, and a bug in
//! the code that parses untrusted traffic cannot reach the worker's NATS
//! credentials or other leases. On Linux the child dies with the worker.
//!
//! On macOS there is no network namespace to hide a socket in, so the proxy
//! listens on a loopback TCP port instead. Any local process can open a
//! connection to it; the token is what keeps it from being used.
//!
//! On Windows the proxy listens on a loopback port inside the range the
//! installed fence lets the sandbox account reach, which is the only
//! destination that account can connect to.

#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use anyhow::Context;
#[cfg(unix)]
use nix::libc;
use tokio::process::{Child, Command};
use trogon_sandbox_contract::{NetworkPolicy, TenantId};
use trogon_sandbox_egress_proxy::ingress::ListenAddress;
use trogon_sandbox_protocol::RegistryHandoff;
use trogon_std::env::{ReadEnv, SystemEnv};

#[cfg(target_os = "linux")]
use crate::constants::SOCKET_NAME;
use crate::constants::{CA_FILE_NAME, POLICY_FILE_NAME, READY_POLL, READY_TIMEOUT};
#[cfg(target_os = "linux")]
use crate::isolation::relay::RelayEgress;
use crate::network_policy_needs::NetworkPolicyNeeds;
pub use crate::proxy_env::LeaseProxyToken;
use crate::proxy_env::{CaCertificate, ProxyCredentials, ProxyEndpoint, ProxyEnvironment};

/// The broker a `Transform` rule in the proxy fetches its credentials from.
#[derive(Debug, Clone)]
pub struct BrokerEndpoint {
    pub nats_url: String,
    pub tenant: TenantId,
    /// The committed policy revision the lease's credential fetches carry.
    pub fence: Option<trogon_sandbox_contract::PolicyFence>,
}

/// What a lease proxy needs beyond its policy.
///
/// `secrets` travels to the child over an inherited pipe: only the descriptor
/// number is in the child's environment, so the values are never on argv, in
/// `/proc/<pid>/environ`, or in a file.
#[derive(Debug, Default)]
pub struct LeaseProxyExtras {
    pub secrets: Option<RegistryHandoff>,
    pub upstream_ca_bundle: Option<PathBuf>,
    pub broker: Option<BrokerEndpoint>,
    pub ca: Option<CaFilePaths>,
}

/// The PEM files of a CA the proxy signs with instead of a generated one.
#[derive(Debug, Clone)]
pub struct CaFilePaths {
    pub cert: PathBuf,
    pub key: PathBuf,
}

struct ProxyPaths {
    policy_file: PathBuf,
    ca_file: PathBuf,
    listen: ListenAddress,
}

pub struct LeaseProxy {
    _dir: tempfile::TempDir,
    #[cfg(target_os = "linux")]
    socket: PathBuf,
    #[cfg(any(target_os = "macos", windows))]
    loopback: std::net::SocketAddr,
    token: LeaseProxyToken,
    ca_file: Option<PathBuf>,
    child: Child,
}

impl LeaseProxy {
    #[cfg(unix)]
    pub async fn spawn(proxy_binary: &Path, policy: &NetworkPolicy) -> anyhow::Result<Self> {
        Self::spawn_with(proxy_binary, policy, LeaseProxyExtras::default()).await
    }

    #[cfg(unix)]
    pub async fn spawn_with(
        proxy_binary: &Path,
        policy: &NetworkPolicy,
        extras: LeaseProxyExtras,
    ) -> anyhow::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("sandbox-egress-")
            .tempdir()
            .context("creating the lease proxy directory")?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .context("restricting the lease proxy directory to its owner")?;
        let ingress = Ingress::allocate(dir.path())?;
        Self::start(proxy_binary, policy, extras, dir, ingress).await
    }

    /// Starts a proxy that listens on a free loopback port inside `ports`.
    #[cfg(windows)]
    pub async fn spawn_fenced(
        proxy_binary: &Path,
        policy: &NetworkPolicy,
        ports: trogon_sandbox_windows_fence::ProxyPortRange,
        extras: LeaseProxyExtras,
    ) -> anyhow::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("sandbox-egress-")
            .tempdir()
            .context("creating the lease proxy directory")?;
        let ingress = Ingress::allocate(ports)?;
        Self::start(proxy_binary, policy, extras, dir, ingress).await
    }

    async fn start(
        proxy_binary: &Path,
        policy: &NetworkPolicy,
        #[cfg_attr(not(unix), allow(unused_mut))] mut extras: LeaseProxyExtras,
        dir: tempfile::TempDir,
        ingress: Ingress,
    ) -> anyhow::Result<Self> {
        let paths = ProxyPaths {
            policy_file: dir.path().join(POLICY_FILE_NAME),
            ca_file: dir.path().join(CA_FILE_NAME),
            listen: ingress.listen,
        };
        std::fs::write(&paths.policy_file, serde_json::to_vec(policy)?).context("writing the lease policy")?;
        let token = LeaseProxyToken::generate()?;
        let expose_ca = policy.requires_tls_intercept() || extras.secrets.is_some();

        #[cfg(unix)]
        let (reader, writer) = match extras.secrets.take() {
            Some(handoff) => {
                let (reader, writer) = std::io::pipe().context("creating the secret handoff")?;
                (Some(reader), Some((writer, handoff)))
            }
            None => (None, None),
        };
        #[cfg(unix)]
        let secrets_fd = reader.as_ref().map(AsRawFd::as_raw_fd);
        #[cfg(not(unix))]
        let secrets_fd = {
            anyhow::ensure!(
                extras.secrets.is_none(),
                "secrets cannot be handed to a lease proxy on Windows"
            );
            None
        };

        let mut command = proxy_command(proxy_binary, &paths, &token, &extras, secrets_fd, &SystemEnv);
        let child = command
            .spawn()
            .with_context(|| format!("starting {}", proxy_binary.display()))?;
        #[cfg(unix)]
        drop(reader);
        drop(command);

        #[cfg(unix)]
        let handoff_written = writer.map(|(mut writer, handoff)| {
            tokio::task::spawn_blocking(move || writer.write_all(handoff.expose_wire().as_bytes()))
        });

        let mut proxy = Self {
            _dir: dir,
            #[cfg(target_os = "linux")]
            socket: ingress.socket,
            #[cfg(any(target_os = "macos", windows))]
            loopback: ingress.loopback,
            token,
            ca_file: expose_ca.then_some(paths.ca_file),
            child,
        };
        proxy.wait_until_serving().await?;
        #[cfg(unix)]
        if let Some(written) = handoff_written {
            written
                .await
                .context("the secret handoff task failed")?
                .context("handing the secrets to the lease proxy")?;
        }
        Ok(proxy)
    }

    pub fn token(&self) -> &LeaseProxyToken {
        &self.token
    }

    #[cfg(target_os = "linux")]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    #[cfg(any(target_os = "macos", windows))]
    pub fn loopback_addr(&self) -> std::net::SocketAddr {
        self.loopback
    }

    pub fn environment(&self) -> ProxyEnvironment {
        #[cfg(target_os = "linux")]
        let endpoint = ProxyEndpoint::relay_in_a_root_without_hosts();
        #[cfg(any(target_os = "macos", windows))]
        let endpoint = ProxyEndpoint::loopback(self.loopback);
        ProxyEnvironment::new(
            endpoint,
            ProxyCredentials::new(self.token.clone()),
            self.ca_file.clone().map(CaCertificate::new),
        )
    }

    /// The environment for a proxy signing with a CA the daemon did not write
    /// out itself, whose certificate the sandboxed side can read.
    pub fn environment_trusting(&self, ca_cert: PathBuf) -> ProxyEnvironment {
        #[cfg(target_os = "linux")]
        let endpoint = ProxyEndpoint::relay_in_a_root_without_hosts();
        #[cfg(any(target_os = "macos", windows))]
        let endpoint = ProxyEndpoint::loopback(self.loopback);
        ProxyEnvironment::new(
            endpoint,
            ProxyCredentials::new(self.token.clone()),
            Some(CaCertificate::new(ca_cert)),
        )
    }

    #[cfg(target_os = "linux")]
    pub fn relay_egress(&self) -> std::io::Result<RelayEgress> {
        RelayEgress::with_current_binary(self.socket.clone())
    }

    pub async fn exited(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait().await
    }

    #[cfg(target_os = "linux")]
    fn is_serving(&self) -> bool {
        socket_is_owner_only(&self.socket)
    }

    #[cfg(any(target_os = "macos", windows))]
    fn is_serving(&self) -> bool {
        std::net::TcpStream::connect_timeout(&self.loopback, READY_POLL).is_ok()
    }

    async fn wait_until_serving(&mut self) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait()? {
                anyhow::bail!("the lease proxy exited before serving: {status}");
            }
            if self.is_serving() {
                return Ok(());
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "the lease proxy did not start serving within {READY_TIMEOUT:?}"
            );
            tokio::time::sleep(READY_POLL).await;
        }
    }
}

#[cfg(target_os = "linux")]
struct Ingress {
    listen: ListenAddress,
    socket: PathBuf,
}

#[cfg(target_os = "linux")]
impl Ingress {
    fn allocate(dir: &Path) -> anyhow::Result<Self> {
        let socket = dir.join(SOCKET_NAME);
        Ok(Self {
            listen: ListenAddress::Unix(socket.clone()),
            socket,
        })
    }
}

#[cfg(any(target_os = "macos", windows))]
struct Ingress {
    listen: ListenAddress,
    loopback: std::net::SocketAddr,
}

#[cfg(target_os = "macos")]
impl Ingress {
    /// Picks a free loopback port by binding and releasing it. Another
    /// process could take the port before the proxy does; the proxy then
    /// fails to start and the launch fails, never silently.
    fn allocate(_dir: &Path) -> anyhow::Result<Self> {
        let probe =
            std::net::TcpListener::bind(("127.0.0.1", 0)).context("reserving a loopback port for the lease proxy")?;
        let loopback = probe.local_addr()?;
        drop(probe);
        Ok(Self {
            listen: ListenAddress::Tcp(loopback.to_string()),
            loopback,
        })
    }
}

#[cfg(windows)]
impl Ingress {
    /// Picks a free loopback port of the fence's range by binding and
    /// releasing it. Another process could take the port before the proxy
    /// does; the proxy then fails to start and the launch fails, never
    /// silently.
    fn allocate(ports: trogon_sandbox_windows_fence::ProxyPortRange) -> anyhow::Result<Self> {
        for port in ports.low()..=ports.high() {
            if let Ok(probe) = std::net::TcpListener::bind(("127.0.0.1", port)) {
                let loopback = probe.local_addr()?;
                drop(probe);
                return Ok(Self {
                    listen: ListenAddress::Tcp(loopback.to_string()),
                    loopback,
                });
            }
        }
        anyhow::bail!("every loopback port of the fence range {ports} is in use")
    }
}

fn proxy_command(
    proxy_binary: &Path,
    paths: &ProxyPaths,
    token: &LeaseProxyToken,
    extras: &LeaseProxyExtras,
    secrets_fd: Option<i32>,
    env: &impl ReadEnv,
) -> Command {
    let mut command = Command::new(proxy_binary);
    command
        .env_clear()
        .env("EGRESS_PROXY_LISTEN_ADDR", "off")
        .env("EGRESS_PROXY_POLICY_FILE", &paths.policy_file)
        .env("EGRESS_PROXY_MUX_LISTEN_ADDR", paths.listen.to_string())
        .env("EGRESS_PROXY_CA_OUT_FILE", &paths.ca_file)
        .env("EGRESS_PROXY_MUX_TOKEN", token.expose())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .envs(trogon_sandbox_egress_proxy::network_env::forwarded(|name| {
            env.var(name).ok()
        }));
    if let Some(bundle) = &extras.upstream_ca_bundle {
        command.env("SSL_CERT_FILE", bundle);
    }
    if let Some(ca) = &extras.ca {
        command
            .env("EGRESS_PROXY_CA_CERT_FILE", &ca.cert)
            .env("EGRESS_PROXY_CA_KEY_FILE", &ca.key);
    }
    if let Some(broker) = &extras.broker {
        command
            .env("EGRESS_PROXY_CREDENTIAL_BROKER_NATS_URL", &broker.nats_url)
            .env("EGRESS_PROXY_CREDENTIAL_BROKER_TENANT_ID", broker.tenant.as_str());
        if let Some(fence) = &broker.fence {
            command
                .env("EGRESS_PROXY_CREDENTIAL_BROKER_LEASE_ID", fence.lease_id.as_str())
                .env(
                    "EGRESS_PROXY_CREDENTIAL_BROKER_POLICY_REVISION",
                    fence.policy_revision.to_string(),
                );
        }
    }
    if let Some(fd) = secrets_fd {
        command.env("EGRESS_PROXY_SECRETS_FD", fd.to_string());
    }
    #[cfg(windows)]
    for name in ["SystemRoot", "windir", "SystemDrive"] {
        if let Some(value) = env.var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(unix)]
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if let Some(fd) = secrets_fd
                && libc::fcntl(fd, libc::F_SETFD, 0) == -1
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

#[cfg(target_os = "linux")]
fn socket_is_owner_only(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.file_type().is_socket() && meta.permissions().mode() & 0o777 == 0o600)
}

#[cfg(test)]
mod tests;
