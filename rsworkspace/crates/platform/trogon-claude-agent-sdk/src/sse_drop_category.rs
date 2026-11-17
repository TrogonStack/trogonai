//! Why the browser transport dropped a frame.

/// The reason a frame was dropped, counted per reason by the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SseDropCategory {
    /// A `catch_up_truncated` event: the server did not replay every event after the resume cursor.
    CatchUpTruncated,
    /// A `delivery_update` whose data is not a valid update.
    MalformedDeliveryUpdate,
    /// A frame whose payload has no string `type`.
    MissingType,
    /// An ephemeral frame of a type the transport does not accept.
    EphemeralType,
    /// A `control_response` without a `response.request_id` string.
    MalformedControlResponse,
    /// A user frame from a non-worker source that has the shape of worker output.
    UserWorkerOnly,
    /// A user frame whose content array holds a malformed element.
    MalformedUserContent,
    /// A control response from a non-worker source for a request this client made.
    ForgedReply,
    /// A control request echoed back for one this client made.
    ExpectedEcho,
    /// A durable frame of a type only the worker may send, from another source.
    NonWorkerType,
    /// A frame whose data could not be handled.
    UnparseableFrame,
}

impl SseDropCategory {
    /// The category name the transport reports in its drop counts.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CatchUpTruncated => "catch_up_truncated",
            Self::MalformedDeliveryUpdate => "malformed_delivery_update",
            Self::MissingType => "missing_type",
            Self::EphemeralType => "ephemeral_type",
            Self::MalformedControlResponse => "malformed_control_response",
            Self::UserWorkerOnly => "user_worker_only",
            Self::MalformedUserContent => "malformed_user_content",
            Self::ForgedReply => "forged_reply",
            Self::ExpectedEcho => "expected_echo",
            Self::NonWorkerType => "non_worker_type",
            Self::UnparseableFrame => "unparseable_frame",
        }
    }
}

#[cfg(test)]
#[path = "sse_drop_category_tests.rs"]
mod tests;
