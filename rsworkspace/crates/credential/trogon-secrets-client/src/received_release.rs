use trogonai_proto::secrets::exchange::v1alpha1::ResolveForReleaseRequest;

/// One release request a [`FakeSecretsService`](crate::FakeSecretsService)
/// received: the subject it was asked on and the payload, decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedRelease {
    subject: String,
    request: ResolveForReleaseRequest,
}

impl ReceivedRelease {
    pub(crate) fn new(subject: String, request: ResolveForReleaseRequest) -> Self {
        Self { subject, request }
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn request(&self) -> &ResolveForReleaseRequest {
        &self.request
    }
}
