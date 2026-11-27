use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_nats::{Client, Message};
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use trogon_sandbox_protocol::subscribe_verified;

use crate::byte_budget::ByteBudget;
pub use crate::constants::MAX_SESSION_TTL;
use crate::ended::Ended;
use crate::reserved::Reserved;
use crate::session_ctx::SessionCtx;
use crate::session_refusal::SessionRefusalError;
use crate::session_table::SessionTable;
use crate::subjects::{SSH_ATTACH_FILTER, SSH_DETACH_FILTER, ssh_stream_to_worker};
use crate::{
    BridgeLimits, SshAttachRequest, SshAttachResponse, SshDetachRequest, SshDetachResponse, SshStreamFrame,
    SshTerminalHost, TunnelError,
};

/// Serves the worker side of the SSH stream for one worker.
///
/// The bridge is provider-agnostic. A worker supplies an [`SshTerminalHost`] and the bridge does the rest: it owns the
/// session table, subscribes to each session's `to_worker` subject, opens one terminal per SSH channel, pumps the
/// terminal's output back on `from_worker`, and enforces the bounds.
///
/// A session is bound to exactly one lease at attach time, and only a worker that holds that lease accepts the attach.
/// Every terminal request the bridge makes uses the bound lease; no frame can name another.
pub struct SshStreamBridge {
    host: Arc<dyn SshTerminalHost>,
    client: Client,
    limits: BridgeLimits,
    sessions: SessionTable<CancellationToken>,
}

impl SshStreamBridge {
    pub fn new(host: Arc<dyn SshTerminalHost>, client: Client, limits: BridgeLimits) -> Arc<Self> {
        Arc::new(Self {
            host,
            client,
            sessions: SessionTable::new(limits.max_sessions),
            limits,
        })
    }

    /// Number of sessions currently bridged.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Binds `request.session_id` to `request.lease_id` and starts serving its stream. `None` when this worker does
    /// not hold the lease: another worker answers.
    pub async fn attach(self: &Arc<Self>, request: &SshAttachRequest) -> Option<SshAttachResponse> {
        if !self.host.holds(&request.lease_id).await {
            return None;
        }
        let cancel = CancellationToken::new();
        match self
            .sessions
            .reserve(&request.session_id, &request.lease_id, || cancel.clone())
        {
            Err(refusal) => return Some(SshAttachResponse::rejected(refusal.to_string())),
            Ok(Reserved::Existing) => return Some(SshAttachResponse::accepted()),
            Ok(Reserved::New) => {}
        }
        let subscription = match self.client.subscribe(ssh_stream_to_worker(&request.session_id)).await {
            Ok(subscription) => subscription,
            Err(error) => {
                self.sessions.remove(&request.session_id);
                return Some(SshAttachResponse::rejected(format!("stream subscribe failed: {error}")));
            }
        };
        let ctx = Arc::new(SessionCtx {
            session_id: request.session_id.clone(),
            lease: request.lease_id.clone(),
            client: self.client.clone(),
            host: Arc::clone(&self.host),
            channels: Mutex::new(HashMap::new()),
            activity: Mutex::new(Instant::now()),
            max_channels: self.limits.max_channels,
        });
        let ttl = Duration::from_secs(u64::from(request.ttl_seconds.get())).min(MAX_SESSION_TTL);
        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            let ended = bridge.run(&ctx, subscription, cancel, ttl).await;
            ctx.close_all(ended.reason()).await;
            bridge.sessions.remove(&ctx.session_id);
            debug!(session_id = %ctx.session_id, "ssh stream session ended");
        });
        Some(SshAttachResponse::accepted())
    }

    /// Ends a session and closes its terminals. `None` when this worker does not hold the lease. Detaching an unknown
    /// session succeeds; detaching one that belongs to another lease is refused.
    pub async fn detach(&self, request: &SshDetachRequest) -> Option<SshDetachResponse> {
        if !self.host.holds(&request.lease_id).await {
            return None;
        }
        Some(match self.sessions.get(&request.session_id) {
            None => SshDetachResponse::accepted(),
            Some((lease, _)) if lease != request.lease_id => {
                SshDetachResponse::rejected(SessionRefusalError::BoundToAnotherLease.to_string())
            }
            Some((_, cancel)) => {
                cancel.cancel();
                SshDetachResponse::accepted()
            }
        })
    }

    async fn run(
        &self,
        ctx: &Arc<SessionCtx>,
        mut subscription: async_nats::Subscriber,
        cancel: CancellationToken,
        ttl: Duration,
    ) -> Ended {
        let budget = ByteBudget::new(self.limits.max_buffered_bytes);
        let (tx, rx) = mpsc::unbounded_channel::<(SshStreamFrame, OwnedSemaphorePermit)>();
        let idle_timeout = self.limits.idle_timeout;

        // The processor writes frames to the terminals one at a time. Each frame's bytes stay charged to the budget
        // until it has been written.
        let processor_ctx = Arc::clone(ctx);
        let queued = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
        let processor = tokio::spawn(queued.for_each(move |(frame, permit)| {
            let ctx = Arc::clone(&processor_ctx);
            async move {
                let _permit = permit;
                ctx.handle(frame).await;
            }
        }));

        let reader = async {
            loop {
                let Some(message) = subscription.next().await else {
                    return Ended::Closed;
                };
                let Ok(frame) = serde_json::from_slice::<SshStreamFrame>(&message.payload) else {
                    continue;
                };
                if frame.session_id != ctx.session_id {
                    continue;
                }
                ctx.touch();
                let Some(permit) = budget.take(frame.data.len()) else {
                    return Ended::InputOverflow;
                };
                drop(tx.send((frame, permit)));
            }
        };
        let idle = async {
            loop {
                tokio::time::sleep_until((ctx.last_activity() + idle_timeout).into()).await;
                if ctx.last_activity() + idle_timeout <= Instant::now() {
                    return Ended::Idle;
                }
            }
        };
        let ended = tokio::select! {
            _ = cancel.cancelled() => Ended::Detached,
            ended = reader => ended,
            ended = idle => ended,
            _ = tokio::time::sleep(ttl) => Ended::Expired,
        };
        processor.abort();
        ended
    }

    /// Answers `sandbox.ssh.attach.>` and `sandbox.ssh.detach.>` until `cancel`, for a worker with no attach handling
    /// of its own.
    pub async fn serve(self: Arc<Self>, client: Client, cancel: CancellationToken) -> Result<(), TunnelError> {
        let mut attach = subscribe_verified(&client, SSH_ATTACH_FILTER).await?;
        let mut detach = subscribe_verified(&client, SSH_DETACH_FILTER).await?;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                message = attach.next() => {
                    let Some(message) = message else { return Ok(()) };
                    let bridge = Arc::clone(&self);
                    tokio::spawn(answer(client.clone(), message, move |request| async move {
                        bridge.attach(&request).await
                    }));
                }
                message = detach.next() => {
                    let Some(message) = message else { return Ok(()) };
                    let bridge = Arc::clone(&self);
                    tokio::spawn(answer(client.clone(), message, move |request| async move {
                        bridge.detach(&request).await
                    }));
                }
            }
        }
    }
}

/// Decodes `message`, lets `handle` answer it, and replies when it did. A message with no reply subject, an undecodable
/// body, or no answer is dropped: another worker may hold the lease.
async fn answer<Request, Response, Fut>(client: Client, message: Message, handle: impl FnOnce(Request) -> Fut)
where
    Request: DeserializeOwned,
    Response: Serialize,
    Fut: Future<Output = Option<Response>>,
{
    let Some(reply) = message.reply else { return };
    let Ok(request) = serde_json::from_slice::<Request>(&message.payload) else {
        return;
    };
    if let Some(response) = handle(request).await
        && let Ok(body) = serde_json::to_vec(&response)
    {
        drop(client.publish(reply, Bytes::from(body)).await);
    }
}
