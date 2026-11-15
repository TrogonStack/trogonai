use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};

use buffa::Message;
use grpc_nats_micro::{ContentType, EndpointHandler, ServiceFault};
use nkeys::XKey;
use trogonai_proto::secrets::exchange::v1alpha1::ResolveForReleaseRequest;

use super::ReceivedRelease;
use super::fake_answer::FakeAnswer;

/// Answers one release request the way the secrets service's endpoint does:
/// a payload that does not decode is `INVALID_ARGUMENT` on the error channel,
/// and anything else is recorded and answered with the scripted outcome.
pub(crate) struct FakeReleaseHandler<'a> {
    subject: &'a str,
    answer: &'a FakeAnswer,
    service: &'a XKey,
    received: &'a Mutex<Vec<ReceivedRelease>>,
}

impl<'a> FakeReleaseHandler<'a> {
    pub(crate) fn new(
        subject: &'a str,
        answer: &'a FakeAnswer,
        service: &'a XKey,
        received: &'a Mutex<Vec<ReceivedRelease>>,
    ) -> Self {
        Self {
            subject,
            answer,
            service,
            received,
        }
    }

    fn reply(&self, request_bytes: &[u8]) -> Result<Vec<u8>, ServiceFault> {
        let request = ResolveForReleaseRequest::decode_from_slice(request_bytes)
            .map_err(|_| ServiceFault::invalid_argument("release request payload does not decode"))?;
        let response = self.answer.reply(&request, self.service);
        self.received
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ReceivedRelease::new(self.subject.to_owned(), request));
        Ok(response?.encode_to_vec())
    }
}

impl EndpointHandler for FakeReleaseHandler<'_> {
    /// Only protobuf reaches here: the fake's service options accept no other
    /// content type.
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        _content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(std::future::ready(self.reply(request_bytes)))
    }
}
