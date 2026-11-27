use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_egress_proxy::ingress::{ListenAddress, Listener, ProxyToken};
use trogon_sandbox_egress_proxy::server::{BoundAddress, ProxyServer, permitted_domains_for};
use trogon_sandbox_egress_proxy::tls_ca::{SandboxCa, install_default_crypto_provider};

use crate::constants::{PROXY_SHUTDOWN_GRACE, PROXY_USER};
use crate::{LeaseProxyError, ProxyNetwork};

/// The egress proxy that serves one lease's guest and enforces its network policy.
pub(crate) struct LeaseProxy {
    server: ProxyServer,
    pub(crate) port: u16,
    pub(crate) url: String,
}

impl LeaseProxy {
    pub(crate) async fn start(network: &ProxyNetwork, policy: NetworkPolicy) -> Result<Self, LeaseProxyError> {
        install_default_crypto_provider();
        let ca = SandboxCa::generate(permitted_domains_for(&policy)?)?;
        let listener = Listener::bind(&ListenAddress::parse(&format!("{}:0", network.bind_address))).await?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let server = ProxyServer::builder(policy, ca)
            .explicit(listener, ProxyToken::new(&token)?)
            .start()?;
        let Some(BoundAddress::Tcp(address)) = server.explicit_addr() else {
            return Err(LeaseProxyError::NotTcp);
        };
        let port = address.port();
        Ok(Self {
            server,
            port,
            url: format!("http://{PROXY_USER}:{token}@{}:{port}", network.advertised_address),
        })
    }

    pub(crate) async fn shutdown(self) -> Result<(), LeaseProxyError> {
        Ok(self.server.shutdown(PROXY_SHUTDOWN_GRACE).await?)
    }
}
