//! Asks an external decider about destinations the lease's policy neither
//! allows nor denies. A destination the policy denies, or the SSRF floor
//! refuses, is settled before this module is reached and is never asked
//! about. Every failure to obtain a well-formed allow is a deny.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex as AsyncMutex, RwLock, oneshot};
use tokio::time::timeout;
use tracing::warn;
use trogon_sandbox_contract::{
    AddressDenialClass, DecideMethod, DecideRequest, DecideResponse, EgressViolationReason, LeaseId, NetworkPolicy,
    NetworkPolicyUpdateRequest, NetworkPolicyUpdateResponse, VerdictScope,
};
use trogon_sandbox_contract::{Host, Port};
use trogon_sandbox_protocol::subjects::{egress_decide, lease_network_policy_update};

use crate::policy::{Decision, amended};
use crate::resolve::VettedAddr;

mod constants;
pub use constants::{DEFAULT_DECIDE_TIMEOUT, DEFAULT_MAX_IN_FLIGHT, MAX_REPLY_BYTES, POLICY_PUBLISH_TIMEOUT};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DeciderLimits {
    pub timeout: Duration,
    pub max_in_flight: usize,
}

impl Default for DeciderLimits {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_DECIDE_TIMEOUT,
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeciderTransportError {
    #[error("the decider did not answer in time")]
    TimedOut,
    #[error("the decider is unreachable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait DeciderTransport: Send + Sync {
    async fn request(&self, subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError>;
}

pub struct NatsDeciderTransport {
    client: async_nats::Client,
}

impl NatsDeciderTransport {
    pub fn new(client: async_nats::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DeciderTransport for NatsDeciderTransport {
    async fn request(&self, subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        self.client
            .request(subject, payload.into())
            .await
            .map(|message| message.payload.to_vec())
            .map_err(|err| match err.kind() {
                async_nats::client::RequestErrorKind::TimedOut => DeciderTransportError::TimedOut,
                _ => DeciderTransportError::Unavailable(err.to_string()),
            })
    }
}

/// Writes a persisted verdict into the lease's live policy.
#[async_trait]
pub trait PolicyAmender: Send + Sync {
    async fn amend(&self, host: &Host, port: Port, decision: Decision);
}

/// Hands a changed policy to the same path every live policy update takes.
#[async_trait]
pub trait PolicyPublisher: Send + Sync {
    async fn publish(&self, lease: &LeaseId, policy: &NetworkPolicy) -> Result<(), String>;
}

/// Publishes on [`lease_network_policy_update`], where the broker validates
/// the policy, records it and pushes it to the worker that owns the lease,
/// whose policy file the proxy hot reloads.
pub struct NatsPolicyPublisher {
    client: async_nats::Client,
    timeout: Duration,
}

impl NatsPolicyPublisher {
    pub fn new(client: async_nats::Client) -> Self {
        Self {
            client,
            timeout: POLICY_PUBLISH_TIMEOUT,
        }
    }
}

#[async_trait]
impl PolicyPublisher for NatsPolicyPublisher {
    async fn publish(&self, lease: &LeaseId, policy: &NetworkPolicy) -> Result<(), String> {
        let payload = serde_json::to_vec(&NetworkPolicyUpdateRequest {
            lease_id: lease.clone(),
            network_policy: policy.clone(),
        })
        .unwrap_or_default();
        let reply = timeout(
            self.timeout,
            self.client.request(lease_network_policy_update(lease), payload.into()),
        )
        .await
        .map_err(|_| "the broker did not answer in time".to_string())?
        .map_err(|err| err.to_string())?;
        match serde_json::from_slice::<NetworkPolicyUpdateResponse>(&reply.payload) {
            Ok(NetworkPolicyUpdateResponse::Applied) => Ok(()),
            Ok(NetworkPolicyUpdateResponse::Denied(err)) => Err(format!("{err:?}")),
            Err(err) => Err(format!("undecodable reply: {err}")),
        }
    }
}

/// Applies a persisted verdict to the policy this proxy is serving, at once,
/// then publishes the changed policy so the broker's record and the worker's
/// policy file follow. Publishes are serialised and always carry the latest
/// policy, so a slow earlier one cannot land after a later one.
pub struct LivePolicyAmender {
    lease_id: LeaseId,
    policy: Arc<RwLock<NetworkPolicy>>,
    publisher: Arc<dyn PolicyPublisher>,
    publishing: Arc<AsyncMutex<()>>,
}

impl LivePolicyAmender {
    pub fn new(lease_id: LeaseId, policy: Arc<RwLock<NetworkPolicy>>, publisher: Arc<dyn PolicyPublisher>) -> Self {
        Self {
            lease_id,
            policy,
            publisher,
            publishing: Arc::new(AsyncMutex::new(())),
        }
    }
}

#[async_trait]
impl PolicyAmender for LivePolicyAmender {
    async fn amend(&self, host: &Host, port: Port, decision: Decision) {
        {
            let mut live = self.policy.write().await;
            let Some(changed) = amended(&live, host, port, decision) else {
                return;
            };
            *live = changed;
        }

        let lease_id = self.lease_id.clone();
        let policy = Arc::clone(&self.policy);
        let publisher = Arc::clone(&self.publisher);
        let publishing = Arc::clone(&self.publishing);
        tokio::spawn(async move {
            let _ordered = publishing.lock().await;
            let latest = policy.read().await.clone();
            if let Err(err) = publisher.publish(&lease_id, &latest).await {
                warn!(lease_id = %lease_id.as_str(), error = %err, "failed to persist decider verdict to the lease policy");
            }
        });
    }
}

/// What the decider settled for one destination.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Judgment {
    Allow,
    Deny(EgressViolationReason),
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct Question {
    host: Host,
    port: Port,
}

type Waiters = Vec<oneshot::Sender<Judgment>>;

struct Shared {
    lease_id: LeaseId,
    transport: Arc<dyn DeciderTransport>,
    amender: Arc<dyn PolicyAmender>,
    limits: DeciderLimits,
    pending: Mutex<HashMap<Question, Waiters>>,
}

impl Shared {
    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<Question, Waiters>> {
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn ask(&self, question: &Question, method: Option<DecideMethod>) -> Judgment {
        let request = DecideRequest {
            lease_id: self.lease_id.clone(),
            host: question.host.clone(),
            port: question.port,
            method,
        };
        let payload = serde_json::to_vec(&request).unwrap_or_default();

        let reply = match timeout(
            self.limits.timeout,
            self.transport.request(egress_decide(&self.lease_id), payload),
        )
        .await
        {
            Err(_) | Ok(Err(DeciderTransportError::TimedOut)) => {
                return Judgment::Deny(EgressViolationReason::DeciderTimedOut);
            }
            Ok(Err(DeciderTransportError::Unavailable(err))) => {
                warn!(lease_id = %self.lease_id.as_str(), error = %err, "decider unreachable");
                return Judgment::Deny(EgressViolationReason::DeciderUnavailable);
            }
            Ok(Ok(reply)) => reply,
        };

        let response = if reply.len() > MAX_REPLY_BYTES {
            None
        } else {
            serde_json::from_slice::<DecideResponse>(&reply).ok()
        };
        match response {
            None => Judgment::Deny(EgressViolationReason::DeciderMalformedReply),
            Some(DecideResponse::Allow { scope }) => {
                self.persist(question, scope, Decision::Allow).await;
                Judgment::Allow
            }
            Some(DecideResponse::Deny { scope }) => {
                self.persist(question, scope, Decision::Deny).await;
                Judgment::Deny(EgressViolationReason::DeciderDenied)
            }
        }
    }

    async fn persist(&self, question: &Question, scope: VerdictScope, decision: Decision) {
        if scope == VerdictScope::Persist {
            self.amender.amend(&question.host, question.port, decision).await;
        }
    }
}

/// Releases everyone waiting on a question when dropped, with a deny unless
/// [`Settlement::settle`] gave a judgment, so a task that dies mid-question
/// cannot leave later callers queued behind it forever.
struct Settlement {
    shared: Arc<Shared>,
    question: Question,
    settled: bool,
}

impl Settlement {
    fn settle(mut self, judgment: Judgment) {
        self.release(judgment);
    }

    fn release(&mut self, judgment: Judgment) {
        self.settled = true;
        let waiters = self.shared.pending().remove(&self.question);
        for waiter in waiters.into_iter().flatten() {
            let _ = waiter.send(judgment);
        }
    }
}

impl Drop for Settlement {
    fn drop(&mut self) {
        if !self.settled {
            self.release(Judgment::Deny(EgressViolationReason::DeciderUnavailable));
        }
    }
}

/// The decider for one lease. At most [`DeciderLimits::max_in_flight`]
/// distinct questions are pending at once, and a further new one is denied
/// without being asked. Callers asking about a destination that is already
/// pending wait for that answer instead of sending another request, and share
/// it, a once-only allow included.
pub struct Decider {
    shared: Arc<Shared>,
}

impl Decider {
    pub fn new(
        lease_id: LeaseId,
        transport: Arc<dyn DeciderTransport>,
        amender: Arc<dyn PolicyAmender>,
        limits: DeciderLimits,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                lease_id,
                transport,
                amender,
                limits,
                pending: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub async fn decide(&self, host: &Host, port: Port, method: Option<DecideMethod>) -> Judgment {
        let question = Question {
            host: host.clone(),
            port,
        };
        let (reply, answer) = oneshot::channel();
        let leads = {
            let mut pending = self.shared.pending();
            if let Some(waiters) = pending.get_mut(&question) {
                waiters.push(reply);
                false
            } else if pending.len() >= self.shared.limits.max_in_flight {
                return Judgment::Deny(EgressViolationReason::DeciderOverloaded);
            } else {
                pending.insert(question.clone(), vec![reply]);
                true
            }
        };

        if leads {
            let settlement = Settlement {
                shared: Arc::clone(&self.shared),
                question: question.clone(),
                settled: false,
            };
            tokio::spawn(async move {
                let judgment = settlement.shared.ask(&question, method).await;
                settlement.settle(judgment);
            });
        }

        answer
            .await
            .unwrap_or(Judgment::Deny(EgressViolationReason::DeciderUnavailable))
    }

    /// Settles a destination the policy left unlisted, for the transparent
    /// path where the connection has already been vetted against the SSRF
    /// floor: a refused address is denied without asking.
    pub async fn decide_unlisted(
        decider: Option<&Decider>,
        vetted: &Result<VettedAddr, AddressDenialClass>,
        sni: Option<&str>,
        dest_ip: std::net::IpAddr,
        dest_port: u16,
    ) -> Result<(), EgressViolationReason> {
        let Some(decider) = decider else {
            return Err(EgressViolationReason::PolicyDenied);
        };
        if let Err(class) = vetted {
            return Err(EgressViolationReason::DestinationAddressDenied(*class));
        }
        let host = match sni.map(Host::new) {
            Some(Ok(host)) => host,
            Some(Err(_)) => return Err(EgressViolationReason::PolicyDenied),
            None => Host::from(dest_ip),
        };
        let Ok(port) = Port::new(dest_port) else {
            return Err(EgressViolationReason::PolicyDenied);
        };
        let method = sni.map(|_| DecideMethod::TlsSni);
        match decider.decide(&host, port, method).await {
            Judgment::Allow => Ok(()),
            Judgment::Deny(reason) => Err(reason),
        }
    }
}

#[cfg(test)]
mod tests;
