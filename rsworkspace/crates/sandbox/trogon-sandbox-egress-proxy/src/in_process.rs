//! Decisions answered by callbacks in the embedding process, with no message
//! bus: an ask callback for destinations no rule decides, and a request
//! callback for the requests of an admitted destination.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{DecideRequest, DecideResponse, HttpMethod, LeaseId, VerdictScope};
use trogon_sandbox_contract::{Host, Port};

use crate::decider::{Decider, DeciderLimits, DeciderTransport, DeciderTransportError, PolicyAmender};
use crate::policy::Decision;
use crate::request_body::{RequestBodyReader, serving_reads};
use crate::request_filter::{RequestDecision, RequestFilter, RequestScheme, RequestView, TargetForms};
use crate::violation_store::{DenialReason, DenialReasons};

/// The destination an ask callback is asked about.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AskQuestion {
    pub host: Host,
    pub port: Port,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AskVerdict {
    Allow,
    Deny(DenialReason),
}

impl From<bool> for AskVerdict {
    fn from(allow: bool) -> Self {
        if allow {
            Self::Allow
        } else {
            Self::Deny(DenialReason::default())
        }
    }
}

/// Answers whether a destination that no rule decided may be reached. Any
/// async closure from [`AskQuestion`] to something convertible to
/// [`AskVerdict`] is one.
#[async_trait]
pub trait AskCallback: Send + Sync {
    async fn ask(&self, question: AskQuestion) -> AskVerdict;
}

#[async_trait]
impl<F, Fut, V> AskCallback for F
where
    F: Fn(AskQuestion) -> Fut + Send + Sync,
    Fut: Future<Output = V> + Send,
    V: Into<AskVerdict> + Send,
{
    async fn ask(&self, question: AskQuestion) -> AskVerdict {
        self(question).await.into()
    }
}

/// A decider transport that answers in process by calling an
/// [`AskCallback`]. Verdicts are for the asking connection only.
pub struct InProcessDeciderTransport {
    callback: Arc<dyn AskCallback>,
    reasons: DenialReasons,
}

impl InProcessDeciderTransport {
    pub fn new(callback: Arc<dyn AskCallback>, reasons: DenialReasons) -> Self {
        Self { callback, reasons }
    }
}

#[async_trait]
impl DeciderTransport for InProcessDeciderTransport {
    async fn request(&self, _subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        let request: DecideRequest =
            serde_json::from_slice(&payload).map_err(|err| DeciderTransportError::Unavailable(err.to_string()))?;
        let verdict = self
            .callback
            .ask(AskQuestion {
                host: request.host.clone(),
                port: request.port,
            })
            .await;
        let response = match verdict {
            AskVerdict::Allow => DecideResponse::Allow {
                scope: VerdictScope::Once,
            },
            AskVerdict::Deny(reason) => {
                self.reasons
                    .remember(&request.host.to_string(), request.port.get(), reason);
                DecideResponse::Deny {
                    scope: VerdictScope::Once,
                }
            }
        };
        serde_json::to_vec(&response).map_err(|err| DeciderTransportError::Unavailable(err.to_string()))
    }
}

struct DiscardVerdicts;

#[async_trait]
impl PolicyAmender for DiscardVerdicts {
    async fn amend(&self, _host: &Host, _port: Port, _decision: Decision) {}
}

/// The decider for one lease whose questions go to `callback`.
pub fn in_process_decider(
    lease_id: LeaseId,
    callback: Arc<dyn AskCallback>,
    reasons: DenialReasons,
    limits: DeciderLimits,
) -> Decider {
    Decider::new(
        lease_id,
        Arc::new(InProcessDeciderTransport::new(callback, reasons)),
        Arc::new(DiscardVerdicts),
        limits,
    )
}

/// One request as a [`RequestCallback`] receives it. `path` carries no query.
/// `body` reads the request body on demand. `host` and `port` are the
/// target; `request_target` carries the target as spelled, as normalized, and
/// whether the two differ. `sni` is the server name the client sent in its handshake on a terminated
/// tunnel, and `headers` are the client's header lines as received.
#[derive(Debug, Clone)]
pub struct RequestInfo {
    pub host: Host,
    pub port: u16,
    pub method: HttpMethod,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: RequestBodyReader,
    pub scheme: RequestScheme,
    pub sni: Option<String>,
    pub request_target: Option<TargetForms>,
}

/// Answers whether one HTTP request to an admitted destination may go
/// through. Any async closure from [`RequestInfo`] to `bool` or to a
/// [`RequestDecision`] is one. A manual implementation provides `decide`;
/// the older `allow` still works for one that has not moved yet.
#[async_trait]
pub trait RequestCallback: Send + Sync {
    async fn decide(&self, request: RequestInfo) -> RequestDecision {
        #[allow(deprecated)]
        self.allow(request).await.into()
    }

    /// Replaced by [`RequestCallback::decide`]. Without an override it
    /// denies, so an implementation that provides neither fails closed.
    #[deprecated(note = "implement `decide`, which can answer more than allow or deny")]
    async fn allow(&self, _request: RequestInfo) -> bool {
        false
    }
}

#[async_trait]
impl<F, Fut> RequestCallback for F
where
    F: Fn(RequestInfo) -> Fut + Send + Sync,
    Fut: Future + Send,
    Fut::Output: Into<RequestDecision>,
{
    async fn decide(&self, request: RequestInfo) -> RequestDecision {
        self(request).await.into()
    }

    #[allow(deprecated)]
    async fn allow(&self, request: RequestInfo) -> bool {
        self.decide(request).await.is_allow()
    }
}

/// Plugs a [`RequestCallback`] into the request filter hook.
pub struct CallbackRequestFilter {
    callback: Arc<dyn RequestCallback>,
}

impl CallbackRequestFilter {
    pub fn new(callback: Arc<dyn RequestCallback>) -> Self {
        Self { callback }
    }
}

#[async_trait]
impl RequestFilter for CallbackRequestFilter {
    async fn decide(&self, request: &RequestView<'_>) -> RequestDecision {
        serving_reads(request.body, |body| {
            self.callback.decide(RequestInfo {
                host: request.host.clone(),
                port: request.port,
                method: request.method.clone(),
                path: request.path.to_string(),
                headers: request.headers.to_vec(),
                body,
                scheme: request.scheme,
                sni: request.sni.map(str::to_owned),
                request_target: request.forms.cloned(),
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests;
