use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use http_body_util::BodyExt;
use kube::Client;

use super::fake_reply_tests::{FakeBody, FakeReply};
use crate::k8s_ops::RealK8sOps;

/// A request the fake apiserver received.
#[derive(Debug, Clone)]
pub(super) struct SeenRequest {
    pub(super) method: String,
    pub(super) uri: String,
    pub(super) body: String,
}

/// A `kube` client wired to an in-process service that answers from a script,
/// in order, and records what it was asked. An unscripted request is answered
/// with a 500 so a test never waits on the network.
pub(super) struct FakeApiServer {
    replies: Arc<Mutex<VecDeque<FakeReply>>>,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl FakeApiServer {
    pub(super) fn new(replies: Vec<FakeReply>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into())),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn client(&self) -> Client {
        let replies = Arc::clone(&self.replies);
        let seen = Arc::clone(&self.seen);
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let replies = Arc::clone(&replies);
            let seen = Arc::clone(&seen);
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body
                    .collect()
                    .await
                    .map(|collected| collected.to_bytes())
                    .unwrap_or_default();
                seen.lock().unwrap_or_else(PoisonError::into_inner).push(SeenRequest {
                    method: parts.method.to_string(),
                    uri: parts.uri.to_string(),
                    body: String::from_utf8_lossy(&bytes).into_owned(),
                });
                let reply = replies
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .pop_front()
                    .unwrap_or_else(|| FakeReply::failure(500, "InternalError"));
                reply.into_response()
            }
        });
        Client::new(service, "default")
    }

    /// A client whose transport always fails, like an apiserver that is down.
    pub(super) fn unreachable_client() -> Client {
        Client::new(
            tower::service_fn(|_request: http::Request<kube::client::Body>| async {
                Err::<http::Response<FakeBody>, _>(std::io::Error::other("apiserver down"))
            }),
            "default",
        )
    }

    pub(super) fn ops(&self) -> RealK8sOps {
        RealK8sOps::new(self.client())
    }

    pub(super) fn seen(&self) -> Vec<SeenRequest> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
