use std::net::Ipv4Addr;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use trogon_sandbox_contract::{
    CapabilitySupport, CommandExecution, LaunchCancellation, LeaseId, NetworkPolicy, RunCommandRequest,
    RunCommandResponse, StreamedOutput,
};
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer, permitted_domains_for};
use trogon_sandbox_egress_proxy::tls_ca::{SandboxCa, install_default_crypto_provider};

use crate::agent::{DiscardChunks, GuestAgent};
use crate::capability_check::{PolicyRequirements, ProfileSatisfies};
use crate::vm_dir::VmDir;

pub use crate::host_links::NetworkConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Identity {
    token: String,
    subnet: Ipv4Net,
    #[serde(default)]
    namespace_inode: Option<u64>,
}

impl Identity {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.token.len() == 12
                && self
                    .token
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid network identity"
        );
        ensure!(
            self.subnet.prefix_len() == 30 && self.subnet.addr() == self.subnet.network(),
            "invalid network subnet"
        );
        Ok(())
    }
    fn namespace(&self) -> String {
        format!("sbfc-{}", self.token)
    }
    fn host_link(&self) -> String {
        format!("fch{}", self.token)
    }
    fn peer_link(&self) -> String {
        format!("fcp{}", self.token)
    }
    fn host_address(&self) -> Ipv4Addr {
        (u32::from(self.subnet.network()) + 1).into()
    }
    fn peer_address(&self) -> Ipv4Addr {
        (u32::from(self.subnet.network()) + 2).into()
    }
}

mod credentials;
pub use credentials::{BrokerConnector, CredentialRefresh, ForwardIdentity, LeaseCredentials, forwards_requests};

pub struct LeaseNetwork {
    identity: Identity,
    credentials: Option<LeaseCredentials>,
    forward: Option<ForwardIdentity>,
    proxy: Option<ProxyServer>,
    pub proxy_url: String,
    pub policy: NetworkPolicy,
    _address_lock: std::fs::File,
}

async fn command(args: &[String], input: Option<&str>) -> anyhow::Result<String> {
    let mut child = tokio::process::Command::new("ip")
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting iproute2 command")?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .context("missing command stdin")?
            .write_all(input.as_bytes())
            .await?;
    }
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .context("host network command timed out")??;
    ensure!(
        output.status.success(),
        "host network command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).context("host network command returned non-UTF8 output")
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}

fn firewall(identity: &Identity, port: u16) -> String {
    format!(
        r#"table inet sandbox_fc {{
 chain input {{ type filter hook input priority 0; policy drop; }}
 chain output {{ type filter hook output priority 0; policy drop; }}
 chain forward {{ type filter hook forward priority 0; policy drop;
 iifname "tap0" oifname "{peer}" {saddr} ip daddr {host} tcp dport {port} accept;
 iifname "{peer}" oifname "tap0" ip saddr {host} tcp sport {port} ct state established accept;
 }}
}}
table ip sandbox_fc_nat {{ chain postrouting {{ type nat hook postrouting priority srcnat;
 oifname "{peer}" ip daddr {host} tcp dport {port} snat to {peer_ip};
}}
}}
"#,
        peer = identity.peer_link(),
        saddr = guest_guard::saddr_match(crate::guest_link::GUEST_ADDR),
        host = identity.host_address(),
        peer_ip = identity.peer_address()
    )
}

/// Installs the TAP ingress guard and strict reverse-path filtering for `namespace`.
///
/// Must run again whenever `tap0` is recreated, since the netdev hook and the per-device
/// sysctl belong to the old device.
async fn guard_tap(namespace: &str) -> anyhow::Result<()> {
    command(
        &args(&[
            "netns",
            "exec",
            namespace,
            "sysctl",
            "-qw",
            &guest_guard::rp_filter_sysctl(TAP),
        ]),
        None,
    )
    .await?;
    command(
        &args(&["netns", "exec", namespace, "nft", "-f", "-"]),
        Some(&guest_guard::l2_ruleset(TAP, crate::guest_link::GUEST_MAC)),
    )
    .await?;
    Ok(())
}

/// Adds the namespace's dropped-frame count to the exported total before the namespace, and
/// the counter with it, is deleted. Best effort.
async fn export_spoofed(namespace: &str) {
    let json = command(
        &args(&[
            "netns",
            "exec",
            namespace,
            "nft",
            "-j",
            "list",
            "counter",
            "netdev",
            guest_guard::TABLE,
            guest_guard::SPOOF_COUNTER,
        ]),
        None,
    )
    .await;
    if let Ok(frames) = json.and_then(|json| guest_guard::parse_drop_count(&json)) {
        if frames > 0 {
            tracing::warn!(namespace, frames, "the guest sent spoofed frames the TAP guard dropped");
        }
        crate::spoof_metrics::record(frames);
    }
}

async fn cleanup_identity(identity: &Identity) -> anyhow::Result<()> {
    identity.validate()?;
    let namespace = identity.namespace();
    let host = identity.host_link();
    let namespaces = command(&args(&["netns", "list"]), None).await?;
    if namespaces
        .lines()
        .any(|line| line.split_whitespace().next() == Some(&namespace))
    {
        #[cfg(unix)]
        {
            let inode = std::fs::metadata(PathBuf::from("/var/run/netns").join(&namespace))?.ino();
            ensure!(
                identity.namespace_inode == Some(inode),
                "refusing cleanup of a replaced network namespace"
            );
        }
        export_spoofed(&namespace).await;
        command(&args(&["netns", "delete", &namespace]), None).await?;
    }
    let links: Vec<serde_json::Value> = serde_json::from_str(&command(&args(&["-j", "link", "show"]), None).await?)?;
    if let Some(link) = links.iter().find(|link| link["ifname"].as_str() == Some(&host)) {
        ensure!(
            link["ifalias"].as_str() == Some(&format!("sandbox-fc:{}", identity.token)),
            "refusing cleanup of an unrelated host link"
        );
        command(&args(&["link", "delete", &host]), None).await?;
    }
    Ok(())
}

pub async fn recover(dir: &VmDir) -> anyhow::Result<()> {
    let path = dir.as_path().join(MANIFEST);
    let raw = match tokio::fs::read(&path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let identity: Identity = serde_json::from_slice(&raw)?;
    cleanup_identity(&identity).await?;
    tokio::fs::remove_file(path).await?;
    Ok(())
}

pub async fn assign_tap_owner(namespace_path: &std::path::Path, uid: u32, gid: u32) -> anyhow::Result<()> {
    ensure!(
        matches!(
            namespace_path.parent().and_then(|path| path.to_str()),
            Some("/run/netns" | "/var/run/netns")
        ),
        "invalid network namespace path"
    );
    let namespace = namespace_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid network namespace name")?;
    ensure!(
        namespace.starts_with("sbfc-")
            && namespace
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid worker network namespace"
    );
    let links: serde_json::Value =
        serde_json::from_str(&command(&args(&["-n", namespace, "-j", "link", "show", "tap0"]), None).await?)?;
    let mac = links[0]["address"].as_str().context("TAP has no MAC address")?;
    command(
        &args(&[
            "netns", "exec", namespace, "ip", "tuntap", "delete", "dev", "tap0", "mode", "tap",
        ]),
        None,
    )
    .await?;
    command(
        &args(&[
            "netns",
            "exec",
            namespace,
            "ip",
            "tuntap",
            "add",
            "dev",
            "tap0",
            "mode",
            "tap",
            "user",
            &uid.to_string(),
            "group",
            &gid.to_string(),
        ]),
        None,
    )
    .await?;
    command(
        &args(&["-n", namespace, "address", "add", GATEWAY, "dev", "tap0"]),
        None,
    )
    .await?;
    command(&args(&["-n", namespace, "link", "set", "tap0", "address", mac]), None).await?;
    command(&args(&["-n", namespace, "link", "set", "tap0", "up"]), None).await?;
    guard_tap(namespace).await?;
    Ok(())
}

impl LeaseNetwork {
    pub fn namespace_path(&self) -> PathBuf {
        PathBuf::from("/var/run/netns").join(self.identity.namespace())
    }

    pub async fn prepare(
        config: &NetworkConfig,
        capacity: u32,
        dir: &VmDir,
        policy: NetworkPolicy,
        credentials: Option<LeaseCredentials>,
        forward: Option<ForwardIdentity>,
    ) -> anyhow::Result<Self> {
        config.validate(capacity)?;
        let mut random = [0u8; 6];
        getrandom::fill(&mut random)?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let routes: Vec<serde_json::Value> =
            serde_json::from_str(&command(&args(&["-j", "route", "show", "table", "main"]), None).await?)?;
        let mut selected = None;
        for slot in 0..capacity {
            let subnet = Ipv4Net::new((u32::from(config.host_links.network()) + slot * 4).into(), 30)?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(format!("/run/lock/sandbox-fc-network-{}", subnet.network()))?;
            if fs2::FileExt::try_lock_exclusive(&file).is_err() {
                continue;
            }
            let overlaps = routes
                .iter()
                .filter_map(|route| route["dst"].as_str())
                .filter(|route| *route != "default")
                .filter_map(|route| {
                    route
                        .parse::<Ipv4Net>()
                        .ok()
                        .or_else(|| route.parse::<Ipv4Addr>().ok().and_then(|ip| Ipv4Net::new(ip, 32).ok()))
                })
                .any(|route| route.contains(&subnet.network()) || subnet.contains(&route.network()));
            if overlaps {
                continue;
            }
            selected = Some((subnet, file));
            break;
        }
        let (subnet, address_lock) = selected.context("no unused host link subnet is available")?;
        let identity = Identity {
            token,
            subnet,
            namespace_inode: None,
        };
        tokio::fs::write(dir.as_path().join(MANIFEST), serde_json::to_vec(&identity)?).await?;
        let mut network = Self {
            identity,
            credentials,
            forward,
            proxy: None,
            proxy_url: String::new(),
            policy: policy.clone(),
            _address_lock: address_lock,
        };
        if let Err(error) = network.setup(policy, dir).await {
            let _ = network.shutdown(dir).await;
            return Err(error);
        }
        Ok(network)
    }

    async fn setup(&mut self, policy: NetworkPolicy, dir: &VmDir) -> anyhow::Result<()> {
        let namespace = self.identity.namespace();
        let host = self.identity.host_link();
        let peer = self.identity.peer_link();
        command(&args(&["netns", "add", &namespace]), None).await?;
        #[cfg(unix)]
        {
            self.identity.namespace_inode = Some(std::fs::metadata(self.namespace_path())?.ino());
        }
        tokio::fs::write(dir.as_path().join(MANIFEST), serde_json::to_vec(&self.identity)?).await?;
        let identity = &self.identity;
        command(
            &args(&["link", "add", &host, "type", "veth", "peer", "name", &peer]),
            None,
        )
        .await?;
        command(
            &args(&["link", "set", &host, "alias", &format!("sandbox-fc:{}", identity.token)]),
            None,
        )
        .await?;
        command(&args(&["link", "set", &peer, "netns", &namespace]), None).await?;
        command(
            &args(&[
                "address",
                "add",
                &format!("{}/30", identity.host_address()),
                "dev",
                &host,
            ]),
            None,
        )
        .await?;
        command(&args(&["link", "set", &host, "up"]), None).await?;
        command(
            &args(&[
                "-n",
                &namespace,
                "address",
                "add",
                &format!("{}/30", identity.peer_address()),
                "dev",
                &peer,
            ]),
            None,
        )
        .await?;
        command(&args(&["-n", &namespace, "link", "set", &peer, "up"]), None).await?;
        command(
            &args(&[
                "netns", "exec", &namespace, "ip", "tuntap", "add", "dev", "tap0", "mode", "tap", "user", "0", "group",
                "0",
            ]),
            None,
        )
        .await?;
        command(
            &args(&["-n", &namespace, "address", "add", GATEWAY, "dev", "tap0"]),
            None,
        )
        .await?;
        command(&args(&["-n", &namespace, "link", "set", "tap0", "up"]), None).await?;
        command(
            &args(&["netns", "exec", &namespace, "sysctl", "-qw", "net.ipv4.ip_forward=1"]),
            None,
        )
        .await?;
        guard_tap(&namespace).await?;
        install_default_crypto_provider();
        let ca = SandboxCa::generate(permitted_domains_for(&policy)?)?;
        let listener = Listener::bind(&ListenAddress::parse(&format!("{}:0", identity.host_address()))).await?;
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret)?;
        let auth: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut builder = ProxyServer::builder(policy, ca).explicit(listener, ProxyToken::new(&auth)?);
        if let Some(credentials) = &self.credentials {
            builder = builder.credential_source(Arc::clone(&credentials.source));
        }
        if let Some(forward) = &self.forward {
            builder = builder.oidc_identity(forward.context()?);
        }
        let proxy = builder.start()?;
        let port = match proxy.explicit_addr() {
            Some(BoundAddress::Tcp(address)) => address.port(),
            _ => anyhow::bail!("proxy is not TCP"),
        };
        self.proxy_url = format!("http://sandbox:{auth}@{}:{port}", identity.host_address());
        self.proxy = Some(proxy);
        command(
            &args(&["netns", "exec", &namespace, "nft", "-f", "-"]),
            Some(&firewall(identity, port)),
        )
        .await?;
        Ok(())
    }

    pub async fn update_policy(&mut self, policy: &NetworkPolicy) -> anyhow::Result<()> {
        let mut profile = crate::profile::NETWORKED_PROFILE.clone();
        profile.isolation.supports_tls_intercept = CapabilitySupport::Supported;
        profile.isolation.supports_secret_substitution = CapabilitySupport::from(self.credentials.is_some());
        ensure!(
            !forwards_requests(policy) || self.forward.is_some(),
            "request forwarding rules need the worker started with --oidc-signing-key-pem-file"
        );
        ensure!(
            profile.satisfies(&trogon_sandbox_contract::CapabilityRequirements::default().with_network_policy(policy)),
            "network policy requires unavailable Firecracker proxy capabilities"
        );
        if let Some(credentials) = self.credentials.as_mut() {
            credentials.apply(policy).await?;
        }
        let proxy = self
            .proxy
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("lease proxy is not running"))?;
        proxy.replace_policy(policy.clone()).await?;
        self.policy = policy.clone();
        Ok(())
    }

    pub fn guest_trust(&self) -> anyhow::Result<crate::agent::GuestProxyTrust> {
        let proxy = self.proxy.as_ref().context("lease proxy is not running")?;
        crate::agent::GuestProxyTrust::for_certificate(proxy.ca_certificate_pem())
    }

    pub async fn configure_guest(&self, agent: &GuestAgent, lease: &LeaseId) -> anyhow::Result<()> {
        let setup = format!(
            "ip address replace {GUEST} dev eth0 && ip link set eth0 up && ip neighbor flush dev eth0 && ip route replace {host}/32 via 172.16.0.1 dev eth0",
            host = self.identity.host_address()
        );
        let request = RunCommandRequest {
            require_launch_cancellation: LaunchCancellation::NotRequired,
            launch_key: None,
            lease_id: lease.clone(),
            command: vec!["/bin/sh".to_string(), "-c".to_string(), setup],
            cwd: Some("/".to_string()),
            env: Default::default(),
            timeout_seconds: 10,
            stream_output: StreamedOutput::Disabled,
            run_as_user: None,
            output_retention: Default::default(),
            max_output_bytes: None,
            redact_values: Default::default(),
            execution: CommandExecution::Attached,
        };
        match agent.exec(&request, &DiscardChunks).await? {
            RunCommandResponse::Completed(output) if output.exit_code == 0 => {
                let proxy = self.proxy.as_ref().context("lease proxy is not running")?;
                agent.install_proxy_ca(lease, proxy.ca_certificate_pem()).await
            }
            other => anyhow::bail!("guest network setup failed: {other:?}"),
        }
    }

    pub async fn shutdown(&mut self, dir: &VmDir) -> anyhow::Result<()> {
        if let Some(proxy) = self.proxy.take() {
            let _ = proxy.shutdown(Duration::from_secs(2)).await;
        }
        recover(dir).await
    }
}

#[cfg(test)]
mod tests;

pub(crate) mod guest_guard;

#[cfg(test)]
mod tls_tests;

mod constants;
use constants::{GATEWAY, GUEST, MANIFEST, TAP};
