use std::io;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::net::TcpListener;
use trogon_sandbox_contract::{Host, NetworkPolicy, Port};

use super::super::{
    DenyHeaderStyle, IngressLimits, Listener, PermittedDomains, ProxyServer, ProxyServerBuilder, ProxyToken, SandboxCa,
};
use crate::decider::{DeciderTransport, DeciderTransportError, PolicyAmender};
use crate::header_mutator::HeaderMutator;
use crate::ingress::ListenAddress;
use crate::policy::Decision;
use crate::request_filter::{RequestDecision, RequestFilter, RequestView};
use crate::tls_ca::install_default_crypto_provider;
use crate::violation::{ViolationPublishError, ViolationSink};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub fn ca() -> TestResult<SandboxCa> {
    install_default_crypto_provider();
    Ok(SandboxCa::generate(PermittedDomains::from_rules(&[])?)?)
}

pub fn builder(policy: NetworkPolicy) -> TestResult<ProxyServerBuilder> {
    Ok(ProxyServer::builder(policy, ca()?))
}

pub async fn loopback() -> TestResult<Listener> {
    Ok(Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?)
}

pub async fn plain_tcp() -> TestResult<TcpListener> {
    Ok(TcpListener::bind("127.0.0.1:0").await?)
}

pub fn token() -> TestResult<ProxyToken> {
    Ok(ProxyToken::new("lease-token-value")?)
}

pub const STYLES: [DenyHeaderStyle; 2] = [DenyHeaderStyle::ProxyError, DenyHeaderStyle::DenyReason];

pub fn limits() -> IngressLimits {
    IngressLimits::proxy_only()
}

pub struct SilentTransport;

#[async_trait]
impl DeciderTransport for SilentTransport {
    async fn request(&self, _subject: String, _payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        Err(DeciderTransportError::TimedOut)
    }
}

pub struct IgnoringAmender;

#[async_trait]
impl PolicyAmender for IgnoringAmender {
    async fn amend(&self, _host: &Host, _port: Port, _decision: Decision) {}
}

pub struct AllowEverything;

#[async_trait]
impl RequestFilter for AllowEverything {
    async fn decide(&self, _request: &RequestView<'_>) -> RequestDecision {
        RequestDecision::Allow
    }
}

pub struct LeaveHeaders;

impl HeaderMutator for LeaveHeaders {
    fn mutate(&self, _headers: &mut Vec<(String, String)>, _destination: &Host) {}
}

#[derive(Default)]
pub struct CollectingSink(pub Mutex<Vec<Vec<u8>>>);

#[async_trait]
impl ViolationSink for CollectingSink {
    async fn publish(&self, _subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(payload);
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct Captured(pub Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Captured {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|p| p.into_inner())).into_owned()
    }

    pub fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let writer = self.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish()
    }
}
