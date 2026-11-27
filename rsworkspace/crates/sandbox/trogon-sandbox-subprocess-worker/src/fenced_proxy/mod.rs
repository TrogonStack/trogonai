//! The egress proxy of one Windows run, served from a task in this process.
//!
//! The listening socket is bound in the fence's port range and held, so no
//! other process can take the port between choosing it and serving on it. The
//! policy handle a live config update needs is kept too.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_egress_proxy::ingress::{IngressLimits, Listener};
use trogon_sandbox_egress_proxy::{
    PolicyHandle, ProxyServer, ProxyToken, SandboxCa, SecretSubstitution, Sigv4Policies,
    install_default_crypto_provider, permitted_domains_for,
};
use trogon_sandbox_protocol::SentinelRegistry;
use trogon_sandbox_windows_fence::ProxyPortRange;

use crate::attribution::CommandAttribution;
use crate::lease_proxy::{CaFilePaths, LeaseProxyToken};
use crate::network_policy_needs::NetworkPolicyNeeds;
use crate::proxy_env::{CaCertificate, ProxyCredentials, ProxyEndpoint, ProxyEnvironment};

/// The masked credentials the proxy swaps back, which also make it terminate
/// TLS.
pub struct FencedSecrets {
    pub registry: Arc<SentinelRegistry>,
    pub sigv4: Sigv4Policies,
    pub plaintext_inject: bool,
}

pub struct FencedProxy {
    server: ProxyServer,
    token: LeaseProxyToken,
    loopback: SocketAddr,
    ca_file: Option<PathBuf>,
    attribution: CommandAttribution,
    _dir: tempfile::TempDir,
}

impl FencedProxy {
    /// Serves `policy` on the first free loopback port of `ports`, signing
    /// with `ca` when given and with a generated CA otherwise.
    pub async fn start(
        policy: &NetworkPolicy,
        ports: ProxyPortRange,
        ca: Option<&CaFilePaths>,
    ) -> anyhow::Result<Self> {
        Self::start_with(policy, ports, ca, None).await
    }

    /// [`Self::start`] swapping the sentinels of `secrets` back.
    pub async fn start_with(
        policy: &NetworkPolicy,
        ports: ProxyPortRange,
        ca: Option<&CaFilePaths>,
        secrets: Option<FencedSecrets>,
    ) -> anyhow::Result<Self> {
        install_default_crypto_provider();
        let dir = tempfile::Builder::new()
            .prefix("sandbox-egress-")
            .tempdir()
            .context("creating the proxy directory")?;
        let permitted = permitted_domains_for(policy).context("reading the domain rules")?;
        let sandbox_ca = match ca {
            Some(files) => SandboxCa::from_files(&files.cert, &files.key, permitted)
                .map(SandboxCa::permitting_every_name)
                .context("loading the sandbox CA")?,
            None => SandboxCa::generate(permitted).context("generating the sandbox CA")?,
        };
        let ca_file = if ca.is_none() && (policy.requires_tls_intercept() || secrets.is_some()) {
            let path = dir.path().join(CA_FILE_NAME);
            std::fs::write(&path, sandbox_ca.root_cert_pem()).context("writing the sandbox CA certificate")?;
            Some(path)
        } else {
            None
        };

        let std_listener = ports
            .listen(&[])
            .with_context(|| format!("binding the proxy in the fence range {ports}"))?;
        std_listener.set_nonblocking(true)?;
        let loopback = std_listener.local_addr()?;
        let listener = Listener::Tcp(tokio::net::TcpListener::from_std(std_listener)?);

        let token = LeaseProxyToken::generate()?;
        let mut builder = ProxyServer::builder(policy.clone(), sandbox_ca)
            .explicit(listener, ProxyToken::new(token.expose())?)
            .ingress_limits(IngressLimits::proxy_only());
        if let Some(secrets) = secrets {
            builder = builder
                .secrets(SecretSubstitution::new(secrets.registry).with_sigv4_policies(secrets.sigv4))
                .plaintext_inject(secrets.plaintext_inject);
        }
        let server = builder.start().context("starting the egress proxy")?;
        Ok(Self {
            server,
            token,
            loopback,
            ca_file,
            attribution: CommandAttribution::default(),
            _dir: dir,
        })
    }

    /// Names the command in the proxy user name the environment presents
    /// encodes.
    pub fn attributed_to(mut self, attribution: CommandAttribution) -> Self {
        self.attribution = attribution;
        self
    }

    pub fn token(&self) -> &LeaseProxyToken {
        &self.token
    }

    pub fn loopback_addr(&self) -> SocketAddr {
        self.loopback
    }

    pub fn policy_handle(&self) -> PolicyHandle {
        self.server.policy_handle()
    }

    pub fn environment(&self) -> ProxyEnvironment {
        self.environment_with(self.ca_file.clone())
    }

    /// The environment for a proxy signing with a CA whose certificate the
    /// sandboxed side reads from `ca_cert`.
    pub fn environment_trusting(&self, ca_cert: PathBuf) -> ProxyEnvironment {
        self.environment_with(Some(ca_cert))
    }

    fn environment_with(&self, ca: Option<PathBuf>) -> ProxyEnvironment {
        ProxyEnvironment::new(
            ProxyEndpoint::loopback(self.loopback),
            ProxyCredentials::new(self.token.clone()),
            ca.map(CaCertificate::new),
        )
        .with_attribution(&self.attribution)
    }

    pub async fn shutdown(self) {
        if let Err(error) = self.server.shutdown(SHUTDOWN_GRACE).await {
            tracing::warn!(%error, "the egress proxy did not stop cleanly");
        }
    }
}

mod constants;
#[cfg(test)]
mod tests;
use constants::{CA_FILE_NAME, SHUTDOWN_GRACE};
