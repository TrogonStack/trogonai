use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::agent::GuestAgent;
use crate::error::ProviderError;
use crate::vm_dir::VmDir;
use trogon_sandbox_contract::{LeaseId, NetworkPolicy};

pub use crate::guest_link::{
    GUEST_ADDR, GUEST_INTERFACE, GUEST_MAC, HOST_ADDR, PROXY_PORT, TAP_DEVICE, guest_ip_boot_arg, relay_listen_addr,
};
pub use crate::host_links::NetworkConfig;
pub use crate::host_network::{
    BrokerConnector, CredentialRefresh, ForwardIdentity, LeaseCredentials, assign_tap_owner, recover,
};
pub use crate::legacy_network::{Netns, NetnsName, ProxyToken, enforceable, guest_proxy_env, remove_all};

pub enum LeaseNetwork {
    Host(crate::host_network::LeaseNetwork),
    Helper {
        network: crate::legacy_network::LeaseNetwork,
        policy: NetworkPolicy,
    },
}

impl LeaseNetwork {
    pub async fn prepare(
        config: &NetworkConfig,
        capacity: u32,
        dir: &VmDir,
        policy: NetworkPolicy,
        credentials: Option<LeaseCredentials>,
        forward: Option<ForwardIdentity>,
    ) -> anyhow::Result<Self> {
        Ok(Self::Host(
            crate::host_network::LeaseNetwork::prepare(config, capacity, dir, policy, credentials, forward).await?,
        ))
    }
    pub async fn start(
        config: &crate::config::NetworkConfig,
        dir: &VmDir,
        lease: &LeaseId,
        policy: &NetworkPolicy,
        credentials: Option<LeaseCredentials>,
    ) -> Result<Self, ProviderError> {
        Ok(Self::Helper {
            network: crate::legacy_network::LeaseNetwork::start(config, dir, lease, policy, credentials).await?,
            policy: policy.clone(),
        })
    }
    pub fn legacy(&self) -> Option<&crate::legacy_network::LeaseNetwork> {
        match self {
            Self::Helper { network, .. } => Some(network),
            Self::Host(_) => None,
        }
    }
    pub fn namespace_path(&self) -> PathBuf {
        match self {
            Self::Host(network) => network.namespace_path(),
            Self::Helper { network, .. } => PathBuf::from("/var/run/netns").join(network.netns().name().as_str()),
        }
    }
    pub fn proxy_url(&self) -> String {
        match self {
            Self::Host(network) => network.proxy_url.clone(),
            Self::Helper { network, .. } => network.guest_env().remove("HTTP_PROXY").unwrap_or_default(),
        }
    }
    pub fn policy(&self) -> &NetworkPolicy {
        match self {
            Self::Host(network) => &network.policy,
            Self::Helper { policy, .. } => policy,
        }
    }
    pub fn guest_env(&self) -> BTreeMap<String, String> {
        match self {
            Self::Helper { network, .. } => network.guest_env(),
            Self::Host(_) => {
                let mut env = BTreeMap::new();
                for key in [
                    "HTTP_PROXY",
                    "HTTPS_PROXY",
                    "ALL_PROXY",
                    "http_proxy",
                    "https_proxy",
                    "all_proxy",
                ] {
                    env.insert(key.into(), self.proxy_url());
                }
                env.insert("NO_PROXY".into(), "localhost,127.0.0.1,::1".into());
                env.insert("no_proxy".into(), "localhost,127.0.0.1,::1".into());
                env
            }
        }
    }
    pub fn guest_env_with(&self, requested: &BTreeMap<String, String>) -> anyhow::Result<BTreeMap<String, String>> {
        let mut env = self.guest_env();
        env.extend(requested.clone());
        let trust = match self {
            Self::Host(network) => Some(network.guest_trust()?),
            Self::Helper { network, .. } => network
                .ca_certificate_pem()
                .map(crate::agent::GuestProxyTrust::for_certificate)
                .transpose()?,
        };
        if let Some(trust) = trust {
            env.extend(
                trogon_sandbox_protocol::ca_trust_env(&trust.bundle_path())
                    .into_iter()
                    .map(|(key, value)| (key.into(), value)),
            );
        }
        Ok(env)
    }

    pub async fn configure_guest(&self, agent: &GuestAgent, lease: &LeaseId) -> anyhow::Result<()> {
        match self {
            Self::Host(network) => network.configure_guest(agent, lease).await,
            Self::Helper { network, .. } => match network.ca_certificate_pem() {
                Some(pem) => agent.install_proxy_ca(lease, pem).await,
                None => Ok(()),
            },
        }
    }
    pub async fn update_policy(&mut self, policy: &NetworkPolicy) -> anyhow::Result<()> {
        match self {
            Self::Host(network) => network.update_policy(policy).await?,
            Self::Helper {
                network,
                policy: current,
            } => {
                if !network.intercepts_tls() {
                    enforceable(policy).map_err(anyhow::Error::msg)?;
                }
                network.update_policy(policy).await?;
                *current = policy.clone();
            }
        }
        Ok(())
    }
    pub async fn shutdown(&mut self, dir: &VmDir) -> anyhow::Result<()> {
        match self {
            Self::Host(network) => network.shutdown(dir).await,
            Self::Helper { network, .. } => network.shutdown().await.map_err(Into::into),
        }
    }
}

pub async fn recover_helper(config: &crate::config::NetworkConfig, dir: &VmDir) -> anyhow::Result<()> {
    if let Some(lease) = dir.lease_id() {
        crate::legacy_network::Netns::from_config(config, &lease)?
            .down()
            .await?;
    }
    Ok(())
}
