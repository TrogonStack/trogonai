use std::future::Future;
use std::pin::Pin;

use grpc_nats_micro::{ContentType, EndpointHandler, ServiceFault};
use trogonai_proto::secrets::exchange::v1alpha1::ResolveForReleaseRequest;

use super::constants::RELEASE_DEADLINE;
use super::release_reply::ReleaseReply;
use super::{CredentialReleaseError, ReleaseCall, ReleaseCaller, ReleaseCredential};

/// Answers one request on the release endpoint, for
/// [`grpc_nats_micro::answer`] to negotiate and encode.
///
/// The caller is read from the subject before the payload is decoded, and a
/// subject that names no caller is denied without the payload being read. A
/// release still running at [`RELEASE_DEADLINE`] answers `UNAVAILABLE`.
pub struct ReleaseAnswer<'a, R> {
    subject: &'a str,
    release: &'a R,
}

impl<'a, R> ReleaseAnswer<'a, R> {
    pub fn new(subject: &'a str, release: &'a R) -> Self {
        Self { subject, release }
    }
}

impl<R: ReleaseCredential> ReleaseAnswer<'_, R> {
    async fn reply(&self, request_bytes: &[u8], content_type: ContentType) -> Result<ReleaseReply, ServiceFault> {
        let caller = match ReleaseCaller::from_subject(self.subject) {
            Ok(caller) => caller,
            Err(error) => {
                tracing::warn!(subject = self.subject, error = %error, "release subject names no caller; denied");
                return Ok(ReleaseReply::of(Err(CredentialReleaseError::Denied)));
            }
        };
        let wire: ResolveForReleaseRequest = content_type.decode(request_bytes).map_err(|error| {
            tracing::warn!(subject = self.subject, error = %error, "release request payload does not decode");
            ServiceFault::invalid_argument(format!("release request payload does not decode: {error}"))
        })?;
        let call = ReleaseCall::from_wire(&caller, wire).map_err(|error| {
            tracing::warn!(subject = self.subject, error = %error, "release request is invalid");
            ServiceFault::invalid_argument(error.to_string())
        })?;
        let released = tokio::time::timeout(
            RELEASE_DEADLINE,
            self.release.release(call.request(), call.requester_xkey()),
        )
        .await
        .unwrap_or_else(|_| {
            let deadline_ms = RELEASE_DEADLINE.as_millis();
            tracing::error!(
                subject = self.subject,
                deadline_ms,
                "release did not finish within its deadline; answered unavailable"
            );
            Err(CredentialReleaseError::Unavailable)
        });
        Ok(ReleaseReply::of(released))
    }
}

impl<R: ReleaseCredential> EndpointHandler for ReleaseAnswer<'_, R> {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async move { self.reply(request_bytes, content_type).await?.encode(content_type) })
    }
}

#[cfg(test)]
mod tests;
