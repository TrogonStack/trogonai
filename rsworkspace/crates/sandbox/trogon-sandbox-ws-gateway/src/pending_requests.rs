use std::collections::HashSet;

use crate::WsFrame;
use crate::constants::{MAX_PENDING_REQUESTS, MAX_REQUEST_ID_LEN, RESERVED_CONTROL_REQUESTS};

/// Why a request was turned away before it ran.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Refusal {
    InvalidId,
    TooManyRequests,
}

impl Refusal {
    pub(crate) fn into_frame(self, id: String) -> WsFrame {
        match self {
            Self::InvalidId => WsFrame::response_err(
                id,
                "invalid_request_id",
                "request id must be unique while pending and contain 1 to 128 bytes",
            ),
            Self::TooManyRequests => WsFrame::response_err(
                id,
                "too_many_requests",
                "this socket has reached its pending request limit",
            ),
        }
    }
}

/// The requests one socket has in flight.
#[derive(Debug, Default)]
pub(crate) struct PendingRequests {
    ids: HashSet<String>,
    regular: usize,
}

impl PendingRequests {
    /// Checks that `id` may be used now.
    pub(crate) fn check_id(&self, id: &str) -> Result<(), Refusal> {
        if id.is_empty() || id.len() > MAX_REQUEST_ID_LEN || self.ids.contains(id) {
            return Err(Refusal::InvalidId);
        }
        Ok(())
    }

    /// Takes a slot for a concurrent request. Control requests may use the reserved slots.
    pub(crate) fn admit(&mut self, id: &str, control: bool) -> Result<(), Refusal> {
        let in_flight = self.ids.len();
        if in_flight >= MAX_PENDING_REQUESTS
            || (!control && self.regular >= MAX_PENDING_REQUESTS - RESERVED_CONTROL_REQUESTS)
        {
            return Err(Refusal::TooManyRequests);
        }
        self.ids.insert(id.to_string());
        if !control {
            self.regular += 1;
        }
        Ok(())
    }

    /// Frees the slot of a finished request.
    pub(crate) fn finish(&mut self, id: &str, control: bool) {
        self.ids.remove(id);
        if !control {
            self.regular = self.regular.saturating_sub(1);
        }
    }
}
