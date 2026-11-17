use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use async_nats::subject::ToSubject;
use async_nats::{HeaderMap, Message, Subject};
use bytes::Bytes;
use nkeys::XKey;
use trogon_nats::RequestClient;
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};

use super::fake_answer::FakeAnswer;
use super::fake_release_handler::FakeReleaseHandler;
use super::{FakeOutcome, FakeRequestError, ReceivedRelease, ServiceXkey};

/// A scripted stand-in for the secrets service's release endpoint, for tests
/// that drive a [`ReleaseClient`](crate::ReleaseClient) without a bus.
///
/// It is the transport itself: a [`RequestClient`] that answers in process,
/// so a test needs no server and its delays follow tokio's clock. Each reply
/// still goes through the ADR 0016 encoding the real endpoint uses, so the
/// client reads the same bytes it would read from the bus.
///
/// Each request takes the next scripted [`FakeOutcome`]; with none left, the
/// fake denies. A request is recorded as it arrives, so a scripted delay holds
/// back only its reply. A release is sealed with the fake's own xkey, which
/// [`FakeSecretsService::service_xkey`] pins.
#[derive(Clone)]
pub struct FakeSecretsService {
    xkey: Arc<XKey>,
    script: Arc<Mutex<VecDeque<FakeOutcome>>>,
    received: Arc<Mutex<Vec<ReceivedRelease>>>,
}

impl FakeSecretsService {
    pub fn new() -> Self {
        Self {
            xkey: Arc::new(XKey::new()),
            script: Arc::default(),
            received: Arc::default(),
        }
    }

    /// The key a client pins to open what this fake releases.
    pub fn service_xkey(&self) -> ServiceXkey {
        ServiceXkey::of(&self.xkey)
    }

    /// Queues `outcome` behind the ones already scripted.
    pub fn script(&self, outcome: FakeOutcome) {
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(outcome);
    }

    /// Every request whose payload decoded, in the order it arrived.
    pub fn received(&self) -> Vec<ReceivedRelease> {
        self.received.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn next_outcome(&self) -> FakeOutcome {
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(FakeOutcome::denied)
    }

    async fn answer(
        &self,
        subject: Subject,
        headers: &HeaderMap,
        payload: &Bytes,
        answer: &FakeAnswer,
    ) -> Result<Message, FakeRequestError> {
        let handler = FakeReleaseHandler::new(subject.as_str(), answer, &self.xkey, &self.received);
        let options = ServiceOptions {
            content_type: ProtoContentType::CONTENT_TYPE_PROTOBUF.into(),
            ..ServiceOptions::default()
        };
        let reply = grpc_nats_micro::answer(Some(headers), payload, &options, &handler)
            .await
            .map_err(FakeRequestError::Reply)?;
        Ok(Message {
            subject,
            reply: None,
            length: reply.body.len(),
            payload: reply.body,
            headers: Some(reply.headers),
            status: None,
            description: None,
        })
    }
}

impl Default for FakeSecretsService {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestClient for FakeSecretsService {
    type RequestError = FakeRequestError;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Message, FakeRequestError> {
        let outcome = self.next_outcome();
        let delay = outcome.delay();
        let replied = match outcome.into_answer() {
            Some(answer) => self.answer(subject.to_subject(), &headers, &payload, &answer).await,
            None => Err(FakeRequestError::Unreachable),
        };
        tokio::time::sleep(delay).await;
        replied
    }
}

#[cfg(test)]
mod tests;
