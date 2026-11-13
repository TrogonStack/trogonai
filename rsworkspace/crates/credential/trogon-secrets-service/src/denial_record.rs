use super::{DenialReason, ReleaseRequest};

/// The audit record of a refused release, with the reason the requester is
/// never told.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DenialRecord {
    request: ReleaseRequest,
    reason: DenialReason,
}

impl DenialRecord {
    pub fn new(request: ReleaseRequest, reason: DenialReason) -> Self {
        Self { request, reason }
    }

    pub fn request(&self) -> &ReleaseRequest {
        &self.request
    }

    pub fn reason(&self) -> &DenialReason {
        &self.reason
    }
}
