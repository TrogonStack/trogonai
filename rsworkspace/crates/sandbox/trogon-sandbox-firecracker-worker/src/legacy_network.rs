//! A lease's network device: a tap in its own network namespace whose only way out is the
//! lease's egress proxy.
//!
//! The root helper creates the namespace, the tap and an nftables table that drops
//! everything except TCP to the proxy port on the host side of the tap. Firecracker runs
//! inside the namespace behind the worker's relay, which forwards that port to a
//! `sandbox-egress-proxyd` listening on a unix socket in the lease directory. The proxy runs
//! in the host namespace as the worker's user and enforces the lease's `NetworkPolicy`, so
//! the guest reaches what the policy allows and nothing else, and rewriting the policy file
//! changes that while the VM runs.
//!
//! Every namespace holds exactly one VM, so every lease uses the same addresses.

use std::collections::BTreeMap;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::warn;
use trogon_sandbox_contract::{LeaseId, NetworkPolicy};
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener};
use trogon_sandbox_egress_proxy::server::{ProxyServer, permitted_domains_for};
use trogon_sandbox_egress_proxy::tls_ca::{SandboxCa, install_default_crypto_provider};
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::config::{EgressProxyBinary, FirecrackerBinary, NetnsHelper, NetnsPrefix, NetworkConfig, RelayBinary};
use crate::error::ProviderError;
use crate::guest_link::relay_listen_addr;
use crate::host_network::LeaseCredentials;
use crate::relay::RELAY_SUBCOMMAND;
use crate::vm_dir::VmDir;

/// Whether `policy` can be enforced for a guest: the proxy can only inspect TLS for clients
/// that trust its CA, and nothing installs that CA in the guest.
pub fn enforceable(policy: &NetworkPolicy) -> Result<(), String> {
    if crate::capability_check::requires_tls_intercept(policy) {
        return Err(
            "rules that need TLS interception are not supported by the legacy helper network without a credential broker: its proxy does not terminate TLS and the guest does not trust a CA. Start the worker with --credential-broker-nats-url (credential injection) or move to --host-links"
                .to_string(),
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NetnsName(String);

impl NetnsName {
    pub fn for_lease(prefix: &NetnsPrefix, lease_id: &LeaseId) -> Result<Self, ProviderError> {
        let name = format!("{}{}", prefix.as_str(), lease_id.as_str());
        let valid =
            name.len() <= MAX_NETNS_NAME && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(ProviderError::InvalidSpec(format!(
                "lease id {:?} cannot name a network namespace: at most {} characters of ASCII letters, digits, '-' and '_'",
                lease_id.as_str(),
                MAX_NETNS_NAME - prefix.as_str().len()
            )));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A lease namespace, driven through the root helper.
#[derive(Debug, Clone)]
pub struct Netns {
    helper: NetnsHelper,
    name: NetnsName,
}

impl Netns {
    pub fn from_config(config: &NetworkConfig, lease: &LeaseId) -> Result<Self, ProviderError> {
        Ok(Self {
            helper: config.helper.clone(),
            name: NetnsName::for_lease(&config.prefix, lease)?,
        })
    }
    pub async fn up(helper: NetnsHelper, name: NetnsName) -> Result<Self, ProviderError> {
        let netns = Self { helper, name };
        run_helper(&netns.helper, &["up", netns.name.as_str()]).await?;
        Ok(netns)
    }

    /// Kills whatever still runs in the namespace and deletes it. Deleting one that is
    /// already gone is not an error. The guard's drop count is exported first.
    pub async fn down(&self) -> Result<(), ProviderError> {
        export_spoofed(&self.helper, &self.name).await;
        down(&self.helper, &self.name).await
    }

    pub fn name(&self) -> &NetnsName {
        &self.name
    }

    /// `program` run inside the namespace as the worker's own user.
    fn command(&self, program: &std::path::Path) -> Command {
        let mut command = Command::new("sudo");
        command
            .arg("-n")
            .arg(self.helper.as_path())
            .arg("exec")
            .arg(self.name.as_str())
            .arg("--")
            .arg(program);
        command
    }
}

/// Adds the namespace's dropped-frame count to the exported total. Best effort: a namespace
/// that is already gone, or a helper that cannot report, only loses the count.
async fn export_spoofed(helper: &NetnsHelper, name: &NetnsName) {
    let Ok(json) = run_helper(helper, &["spoofed", name.as_str()]).await else {
        return;
    };
    match crate::host_network::guest_guard::parse_drop_count(&json) {
        Ok(frames) => {
            if frames > 0 {
                warn!(
                    netns = name.as_str(),
                    frames, "the guest sent spoofed frames the TAP guard dropped"
                );
            }
            crate::spoof_metrics::record(frames);
        }
        Err(e) => warn!(netns = name.as_str(), error = %e, "unreadable spoofed-frame counter"),
    }
}

/// Judges the reply of `fc-netns check`. A helper installed before the guard existed rejects
/// the subcommand, and one with a different level prints something else.
pub fn check_reply(reply: Result<&str, &str>) -> Result<(), String> {
    match reply {
        Ok(out) if out.trim() == ANTI_SPOOF_REPLY => Ok(()),
        Ok(out) => Err(format!(
            "the root helper reports \"{}\" where \"{ANTI_SPOOF_REPLY}\" is required",
            out.trim()
        )),
        Err(why) => Err(format!("the root helper does not answer `check` ({why})")),
    }
}

/// Refuses a root helper whose `up` predates the anti-spoof guard, since a lease namespace it
/// builds would let the guest forge its source MAC, IP or VLAN tag. The helper is root-owned
/// and installed by the operator, so the worker cannot reinstall it.
pub async fn verify_helper(helper: &NetnsHelper) -> anyhow::Result<()> {
    let reply = run_helper(helper, &["check"]).await.map_err(|e| e.to_string());
    check_reply(reply.as_deref().map_err(String::as_str)).map_err(|why| {
        anyhow::anyhow!(
            "{why}. A helper that predates the guest anti-spoof rules must be replaced with scripts/fc-netns from this release at {helper}, and the worker must run as an unprivileged user that calls it through sudo"
        )
    })
}

async fn down(helper: &NetnsHelper, name: &NetnsName) -> Result<(), ProviderError> {
    run_helper(helper, &["down", name.as_str()]).await.map(drop)
}

/// Deletes every namespace with this worker's prefix. Only safe while the worker holds no
/// lease, since a live lease's namespace has the same prefix.
pub async fn remove_all(network: &NetworkConfig) -> usize {
    let listed = match run_helper(&network.helper, &["list"]).await {
        Ok(listed) => listed,
        Err(e) => {
            warn!(error = %e, "could not list lease network namespaces");
            return 0;
        }
    };
    let mut removed = 0;
    for name in listed.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !name.starts_with(network.prefix.as_str()) {
            continue;
        }
        match down(&network.helper, &NetnsName(name.to_string())).await {
            Ok(()) => removed += 1,
            Err(e) => warn!(netns = name, error = %e, "could not remove a stale network namespace"),
        }
    }
    removed
}

async fn run_helper(helper: &NetnsHelper, args: &[&str]) -> Result<String, ProviderError> {
    let output = Command::new("sudo")
        .arg("-n")
        .arg(helper.as_path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| ProviderError::Network(format!("running sudo -n {helper}: {e}")))?;
    if !output.status.success() {
        return Err(ProviderError::Network(format!(
            "sudo -n {helper} {} exited {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The secret the guest presents to its proxy. Never logged.
#[derive(Clone)]
pub struct ProxyToken(String);

impl ProxyToken {
    pub fn generate() -> Result<Self, ProviderError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| ProviderError::Network(format!("generating the proxy token: {e}")))?;
        Ok(Self(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ProxyToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProxyToken(..)")
    }
}

/// The environment every command in the guest gets, pointing its HTTP clients at the proxy.
pub fn guest_proxy_env(token: &ProxyToken) -> BTreeMap<String, String> {
    let url = format!("http://{PROXY_USER}:{}@{}", token.expose(), relay_listen_addr());
    let mut env = BTreeMap::new();
    for name in ["HTTP_PROXY", "HTTPS_PROXY"] {
        env.insert(name.to_string(), url.clone());
        env.insert(name.to_lowercase(), url.clone());
    }
    env.insert("NO_PROXY".to_string(), NO_PROXY.to_string());
    env.insert("no_proxy".to_string(), NO_PROXY.to_string());
    env
}

/// One `sandbox-egress-proxyd` serving one lease.
struct LeaseProxy {
    child: Child,
    token: ProxyToken,
    policy_file: PathBuf,
}

impl LeaseProxy {
    async fn spawn(
        binary: &EgressProxyBinary,
        dir: &VmDir,
        policy: &NetworkPolicy,
        env: &impl ReadEnv,
    ) -> Result<Self, ProviderError> {
        let policy_file = dir.as_path().join(POLICY_FILE);
        write_policy(&policy_file, policy).await?;
        let socket = proxy_socket(dir);
        let token = ProxyToken::generate()?;
        let log = std::fs::File::create(dir.as_path().join(PROXY_LOG))
            .map_err(|e| ProviderError::Network(format!("creating the proxy log: {e}")))?;
        let log_err = log
            .try_clone()
            .map_err(|e| ProviderError::Network(format!("duplicating the proxy log: {e}")))?;

        let mut command = Command::new(binary.as_path());
        command
            .env_clear()
            .env("EGRESS_PROXY_LISTEN_ADDR", "off")
            .env("EGRESS_PROXY_POLICY_FILE", &policy_file)
            .env("EGRESS_PROXY_MUX_LISTEN_ADDR", format!("unix:{}", socket.display()))
            .env("EGRESS_PROXY_MUX_TOKEN", token.expose())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .kill_on_drop(true)
            .envs(trogon_sandbox_egress_proxy::network_env::forwarded(|name| {
                env.var(name).ok()
            }));
        #[cfg(target_os = "linux")]
        // SAFETY: prctl is async-signal-safe and touches no memory of the parent.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|e| ProviderError::Network(format!("starting {binary}: {e}")))?;

        let mut proxy = Self {
            child,
            token,
            policy_file,
        };
        proxy.wait_until_serving(&socket).await?;
        Ok(proxy)
    }

    async fn wait_until_serving(&mut self, socket: &std::path::Path) -> Result<(), ProviderError> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(ProviderError::Network(format!(
                    "the egress proxy exited before serving: {status}"
                )));
            }
            if std::fs::symlink_metadata(socket).is_ok_and(|m| m.file_type().is_socket()) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ProviderError::Network(format!(
                    "the egress proxy did not start serving within {READY_TIMEOUT:?}"
                )));
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }

    async fn stop(&mut self) {
        if let Err(e) = self.child.kill().await {
            warn!(error = %e, "stopping the egress proxy failed");
        }
    }
}

/// Replaces the file in one rename, so the proxy's reload never reads half a policy.
async fn write_policy(path: &std::path::Path, policy: &NetworkPolicy) -> Result<(), ProviderError> {
    let body =
        serde_json::to_vec(policy).map_err(|e| ProviderError::Network(format!("encoding the network policy: {e}")))?;
    let staged = path.with_extension("json.next");
    tokio::fs::write(&staged, body)
        .await
        .map_err(|e| ProviderError::Network(format!("writing {}: {e}", staged.display())))?;
    tokio::fs::rename(&staged, path)
        .await
        .map_err(|e| ProviderError::Network(format!("replacing {}: {e}", path.display())))
}

fn proxy_socket(dir: &VmDir) -> PathBuf {
    dir.as_path().join(PROXY_SOCKET)
}

/// The egress proxy of a lease that can have credentials injected: the same in-process
/// `ProxyServer` host-links mode runs, listening on the unix socket the relay forwards to. It
/// terminates TLS with a per-lease CA the guest trusts before the lease is ready, and fetches
/// each credential from the broker per request, so the value never enters the guest.
struct InProcessProxy {
    server: Option<ProxyServer>,
    token: ProxyToken,
    credentials: LeaseCredentials,
}

impl InProcessProxy {
    async fn spawn(dir: &VmDir, policy: &NetworkPolicy, credentials: LeaseCredentials) -> Result<Self, ProviderError> {
        let failed = |what: &str, e: &dyn std::fmt::Display| ProviderError::Network(format!("{what}: {e}"));
        install_default_crypto_provider();
        let permitted = permitted_domains_for(policy).map_err(|e| failed("planning the proxy CA", &e))?;
        let ca = SandboxCa::generate(permitted).map_err(|e| failed("generating the proxy CA", &e))?;
        let socket = proxy_socket(dir);
        let listener = Listener::bind(&ListenAddress::parse(&format!("unix:{}", socket.display())))
            .await
            .map_err(|e| failed("binding the proxy socket", &e))?;
        let token = ProxyToken::generate()?;
        let server = ProxyServer::builder(policy.clone(), ca)
            .explicit(
                listener,
                trogon_sandbox_egress_proxy::ingress::ProxyToken::new(token.expose())
                    .map_err(|e| failed("building the proxy token", &e))?,
            )
            .credential_source(std::sync::Arc::clone(&credentials.source))
            .start()
            .map_err(|e| failed("starting the egress proxy", &e))?;
        Ok(Self {
            server: Some(server),
            token,
            credentials,
        })
    }
}

enum Proxy {
    Daemon(LeaseProxy),
    InProcess(InProcessProxy),
}

impl Proxy {
    fn token(&self) -> &ProxyToken {
        match self {
            Self::Daemon(proxy) => &proxy.token,
            Self::InProcess(proxy) => &proxy.token,
        }
    }

    async fn stop(&mut self) {
        match self {
            Self::Daemon(proxy) => proxy.stop().await,
            Self::InProcess(proxy) => {
                if let Some(server) = proxy.server.take()
                    && let Err(e) = server.shutdown(Duration::from_secs(2)).await
                {
                    warn!(error = %e, "stopping the egress proxy failed");
                }
            }
        }
    }
}

/// Everything one lease's network device needs on the host.
pub struct LeaseNetwork {
    netns: Netns,
    proxy: Proxy,
    relay: RelayBinary,
    proxy_socket: PathBuf,
}

impl LeaseNetwork {
    pub async fn start(
        config: &NetworkConfig,
        dir: &VmDir,
        lease_id: &LeaseId,
        policy: &NetworkPolicy,
        credentials: Option<LeaseCredentials>,
    ) -> Result<Self, ProviderError> {
        let name = NetnsName::for_lease(&config.prefix, lease_id)?;
        let netns = Netns::up(config.helper.clone(), name).await?;
        let proxy = match credentials {
            Some(credentials) => InProcessProxy::spawn(dir, policy, credentials)
                .await
                .map(Proxy::InProcess),
            None => LeaseProxy::spawn(&config.proxy_binary, dir, policy, &SystemEnv)
                .await
                .map(Proxy::Daemon),
        };
        match proxy {
            Ok(proxy) => Ok(Self {
                netns,
                proxy,
                relay: config.relay_binary.clone(),
                proxy_socket: proxy_socket(dir),
            }),
            Err(error) => {
                if let Err(e) = netns.down().await {
                    warn!(error = %e, "removing the namespace of a lease whose proxy failed also failed");
                }
                Err(error)
            }
        }
    }

    /// Firecracker inside the namespace, behind the relay to this lease's proxy. The relay
    /// records its own pid in `dir.pid_file()`, because the worker cannot signal `sudo`.
    pub fn vmm_command(&self, firecracker: &FirecrackerBinary, dir: &VmDir) -> Command {
        let mut command = self.netns.command(self.relay.as_path());
        command
            .arg(RELAY_SUBCOMMAND)
            .arg("--listen")
            .arg(relay_listen_addr().to_string())
            .arg("--socket")
            .arg(&self.proxy_socket)
            .arg("--pid-file")
            .arg(dir.pid_file())
            .arg("--")
            .arg(firecracker.as_path())
            .arg("--api-sock")
            .arg(dir.api_socket());
        command
    }

    pub fn guest_env(&self) -> BTreeMap<String, String> {
        guest_proxy_env(self.proxy.token())
    }

    /// Whether this lease's proxy terminates TLS, which is what lets its policy carry rules
    /// that need to see inside a request.
    pub fn intercepts_tls(&self) -> bool {
        matches!(self.proxy, Proxy::InProcess(_))
    }

    /// The PEM of the CA the guest must trust, when the proxy terminates TLS.
    pub fn ca_certificate_pem(&self) -> Option<&str> {
        match &self.proxy {
            Proxy::InProcess(InProcessProxy {
                server: Some(server), ..
            }) => Some(server.ca_certificate_pem()),
            _ => None,
        }
    }

    /// With the separate proxy this takes effect within its reload interval, about a second.
    /// With the in-process proxy it applies before this returns, after connecting the
    /// credential providers the new policy names.
    pub async fn update_policy(&mut self, policy: &NetworkPolicy) -> Result<(), ProviderError> {
        match &mut self.proxy {
            Proxy::Daemon(proxy) => write_policy(&proxy.policy_file, policy).await,
            Proxy::InProcess(proxy) => {
                let failed =
                    |e: &dyn std::fmt::Display| ProviderError::Network(format!("updating the lease policy: {e}"));
                proxy.credentials.apply(policy).await.map_err(|e| failed(&e))?;
                let server = proxy
                    .server
                    .as_ref()
                    .ok_or_else(|| failed(&"the lease proxy is not running"))?;
                server.replace_policy(policy.clone()).await.map_err(|e| failed(&e))
            }
        }
    }

    pub fn netns(&self) -> &Netns {
        &self.netns
    }

    pub async fn shutdown(&mut self) -> Result<(), ProviderError> {
        let stopped = self.netns.down().await;
        self.proxy.stop().await;
        stopped
    }

    pub async fn stop(&mut self) {
        if let Err(e) = self.netns.down().await {
            warn!(netns = self.netns.name().as_str(), error = %e, "removing the lease network namespace failed");
        }
        self.proxy.stop().await;
    }
}

#[cfg(test)]
mod tests;

mod constants;
pub use constants::ANTI_SPOOF_REPLY;
use constants::{
    MAX_NETNS_NAME, NO_PROXY, POLICY_FILE, PROXY_LOG, PROXY_SOCKET, PROXY_USER, READY_POLL, READY_TIMEOUT, TOKEN_BYTES,
};
