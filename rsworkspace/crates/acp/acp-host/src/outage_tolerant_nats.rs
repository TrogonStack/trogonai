use async_nats::subject::ToSubject;
use async_nats::{HeaderMap, Message};
use bytes::Bytes;
use std::time::Duration;
use tokio::time::Instant;
use tracing::debug;
use trogon_nats::{FlushClient, PublishClient, RequestClient};

use crate::constants::{CONNECTED_POLL, RETRY_BACKOFF_MAX, RETRY_BACKOFF_START};

/// What the retry needs from a NATS client beyond sending requests.
pub trait ConnectionAware: RequestClient {
    /// Whether the client holds a live connection to a server right now.
    fn is_connected(&self) -> bool;

    /// Whether `error` is the server saying nobody was subscribed to the
    /// subject. The server sends that status instead of delivering the
    /// message, so the request reached no one and sending it again cannot run
    /// it twice.
    fn is_no_responders(error: &Self::RequestError) -> bool;
}

impl ConnectionAware for async_nats::Client {
    fn is_connected(&self) -> bool {
        self.connection_state() == async_nats::connection::State::Connected
    }

    fn is_no_responders(error: &Self::RequestError) -> bool {
        error.kind() == async_nats::client::RequestErrorKind::NoResponders
    }
}

/// The host's NATS client for the requests its child sends to the client
/// (`session/request_permission`, `fs/*`, `terminal/*`, extension calls), made
/// to ride out a short NATS outage instead of failing the child's call.
///
/// When NATS restarts, this host and the client bridge reconnect on their own
/// schedules, each backing off further with every failed attempt. A request
/// sent in between finds the client's subscription not yet restored, and the
/// server answers "no responders". The request reached no one, so it is sent
/// again, with backoff, for up to `window` after its first send.
///
/// A request the child makes while the host itself is disconnected first
/// waits, also for up to `window`, for the connection to come back, so it is
/// not buffered across the outage while its timeout runs, and its retries
/// start counting once it can actually be sent. Both waits together stay
/// inside the operation timeout the whole call runs under.
///
/// Any other failure is returned at once. A timeout in particular is never
/// retried: the client may have received the request and still be working on
/// it (a person deciding a permission), and sending it again could run it
/// twice. Notifications and flushes pass straight through.
#[derive(Clone)]
pub struct OutageTolerantNats<N> {
    inner: N,
    window: Duration,
}

impl<N> OutageTolerantNats<N> {
    pub fn new(inner: N, window: Duration) -> Self {
        Self { inner, window }
    }
}

/// Waits until `client` is connected or `deadline` passes, whichever is first.
/// async-nats has no public way to await a state change, so this polls.
async fn until_connected(client: &impl ConnectionAware, deadline: Instant) {
    while !client.is_connected() && Instant::now() < deadline {
        tokio::time::sleep(CONNECTED_POLL).await;
    }
}

impl<N: ConnectionAware> RequestClient for OutageTolerantNats<N> {
    type RequestError = N::RequestError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Message, Self::RequestError> {
        let subject = subject.to_subject();
        until_connected(&self.inner, Instant::now() + self.window).await;
        let deadline = Instant::now() + self.window;
        let mut backoff = RETRY_BACKOFF_START;
        loop {
            match self
                .inner
                .request_with_headers(subject.clone(), headers.clone(), payload.clone())
                .await
            {
                Err(error) if N::is_no_responders(&error) && Instant::now() + backoff < deadline => {
                    debug!(%subject, ?backoff, "no responders; the client may still be reconnecting, sending again");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(RETRY_BACKOFF_MAX);
                    until_connected(&self.inner, deadline).await;
                }
                result => return result,
            }
        }
    }
}

impl<N: PublishClient> PublishClient for OutageTolerantNats<N> {
    type PublishError = N::PublishError;

    fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), Self::PublishError>> + Send {
        self.inner.publish_with_headers(subject, headers, payload)
    }
}

impl<N: FlushClient> FlushClient for OutageTolerantNats<N> {
    type FlushError = N::FlushError;

    fn flush(&self) -> impl Future<Output = Result<(), Self::FlushError>> + Send {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests;
