//! Publishes a structured [`EgressViolation`] for each denied connection,
//! rate limited per lease with bounded memory.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use tracing::warn;
use trogon_sandbox_contract::SecretName;
use trogon_sandbox_contract::{EgressDecision, EgressViolation, EgressViolationReason, LeaseId, RefusedRequest};
use trogon_sandbox_protocol::subjects::egress_violation;

use crate::denial::Denial;
use crate::ingress_outcome::IngressOutcome;
use crate::ingress_rejection_error::IngressRejectionError;
use crate::proxy_outcome::ProxyOutcome;

mod constants;
pub use constants::{DEFAULT_MAX_EVENTS_PER_WINDOW, DEFAULT_MAX_TRACKED_LEASES, DEFAULT_RATE_LIMIT_WINDOW};

#[derive(Debug, thiserror::Error)]
#[error("failed to publish egress violation: {0}")]
pub struct ViolationPublishError(pub String);

#[async_trait]
pub trait ViolationSink: Send + Sync {
    async fn publish(&self, subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError>;
}

pub struct NatsViolationSink {
    client: async_nats::Client,
}

impl NatsViolationSink {
    pub fn new(client: async_nats::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ViolationSink for NatsViolationSink {
    async fn publish(&self, subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        self.client
            .publish(subject, payload.into())
            .await
            .map_err(|err| ViolationPublishError(err.to_string()))
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ViolationRateLimit {
    pub max_events_per_window: u32,
    pub window: Duration,
    pub max_tracked_leases: usize,
}

impl Default for ViolationRateLimit {
    fn default() -> Self {
        Self {
            max_events_per_window: DEFAULT_MAX_EVENTS_PER_WINDOW,
            window: DEFAULT_RATE_LIMIT_WINDOW,
            max_tracked_leases: DEFAULT_MAX_TRACKED_LEASES,
        }
    }
}

#[derive(Debug)]
struct Window {
    started: Instant,
    count: u32,
}

/// Fixed-window limiter keyed by lease. Tracks at most
/// [`ViolationRateLimit::max_tracked_leases`] leases; when full, expired
/// windows are evicted first and events for further new leases are dropped.
#[derive(Debug)]
pub struct ViolationRateLimiter {
    limit: ViolationRateLimit,
    windows: HashMap<LeaseId, Window>,
}

impl ViolationRateLimiter {
    pub fn new(limit: ViolationRateLimit) -> Self {
        Self {
            limit,
            windows: HashMap::new(),
        }
    }

    pub fn admit(&mut self, lease: &LeaseId, now: Instant) -> bool {
        let window = self.limit.window;
        if !self.windows.contains_key(lease) && self.windows.len() >= self.limit.max_tracked_leases {
            self.windows
                .retain(|_, w| now.saturating_duration_since(w.started) < window);
            if self.windows.len() >= self.limit.max_tracked_leases {
                return false;
            }
        }

        let entry = self
            .windows
            .entry(lease.clone())
            .or_insert(Window { started: now, count: 0 });
        if now.saturating_duration_since(entry.started) >= window {
            entry.started = now;
            entry.count = 0;
        }
        if entry.count >= self.limit.max_events_per_window {
            return false;
        }
        entry.count += 1;
        true
    }

    pub fn tracked_leases(&self) -> usize {
        self.windows.len()
    }
}

pub struct ViolationReporter {
    lease_id: LeaseId,
    sink: Arc<dyn ViolationSink>,
    limiter: Mutex<ViolationRateLimiter>,
}

impl ViolationReporter {
    pub fn new(lease_id: LeaseId, sink: Arc<dyn ViolationSink>, limit: ViolationRateLimit) -> Self {
        Self {
            lease_id,
            sink,
            limiter: Mutex::new(ViolationRateLimiter::new(limit)),
        }
    }

    pub async fn report_outcome(&self, outcome: &ProxyOutcome, dest: SocketAddr) {
        if let ProxyOutcome::Denied(denial) = outcome {
            self.report(denial, dest).await;
        }
    }

    pub async fn report_ingress(&self, outcome: &IngressOutcome) {
        match outcome {
            IngressOutcome::Proxied {
                target,
                dest_ip,
                outcome: ProxyOutcome::Denied(denial),
            } => self.report_to(denial, target.port().get(), *dest_ip).await,
            IngressOutcome::Rejected(IngressRejectionError::MalformedHost { host, port }) => {
                self.publish_violation(
                    Some(host.clone()),
                    *port,
                    None,
                    EgressViolationReason::MalformedHost,
                    None,
                    None,
                )
                .await;
            }
            _ => {}
        }
    }

    pub async fn report(&self, denial: &Denial, dest: SocketAddr) {
        self.report_to(denial, dest.port(), Some(dest.ip())).await;
    }

    async fn report_to(&self, denial: &Denial, port: u16, dest_ip: Option<IpAddr>) {
        self.publish_violation(
            denial.host.clone(),
            port,
            dest_ip,
            denial.reason,
            None,
            denial.request.clone(),
        )
        .await;
    }

    pub async fn report_sentinel_off_host(&self, host: &str, port: u16, binding: &SecretName) {
        self.publish_violation(
            Some(host.to_string()),
            port,
            None,
            EgressViolationReason::SecretSentinelOffHost,
            Some(binding.clone()),
            None,
        )
        .await;
    }

    pub async fn report_compressed_response(&self, host: &str, port: u16, binding: &SecretName) {
        self.publish_violation(
            Some(host.to_string()),
            port,
            None,
            EgressViolationReason::SecretCompressedResponse,
            Some(binding.clone()),
            None,
        )
        .await;
    }

    pub async fn report_sigv4_refused(
        &self,
        host: &str,
        port: u16,
        binding: &SecretName,
        reason: EgressViolationReason,
    ) {
        self.publish_violation(Some(host.to_string()), port, None, reason, Some(binding.clone()), None)
            .await;
    }

    pub async fn report_request_denied(&self, host: &str, port: u16, request: RefusedRequest) {
        self.publish_violation(
            Some(host.to_string()),
            port,
            None,
            EgressViolationReason::RequestDenied,
            None,
            Some(request),
        )
        .await;
    }

    async fn publish_violation(
        &self,
        host: Option<String>,
        port: u16,
        dest_ip: Option<IpAddr>,
        reason: EgressViolationReason,
        binding: Option<SecretName>,
        request: Option<RefusedRequest>,
    ) {
        let admitted = self
            .limiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .admit(&self.lease_id, Instant::now());
        if !admitted {
            return;
        }

        let violation = EgressViolation {
            lease_id: self.lease_id.clone(),
            host,
            port,
            dest_ip,
            decision: EgressDecision::Deny,
            reason,
            binding,
            request,
            at: SystemTime::now(),
        };
        self.emit(violation).await;
    }

    async fn emit(&self, violation: EgressViolation) {
        let payload = match serde_json::to_vec(&violation) {
            Ok(payload) => payload,
            Err(err) => {
                warn!(error = %err, "failed to serialize egress violation");
                return;
            }
        };
        if let Err(err) = self.sink.publish(egress_violation(&self.lease_id), payload).await {
            warn!(error = %err, "failed to publish egress violation");
        }
    }
}

#[cfg(test)]
mod tests;
