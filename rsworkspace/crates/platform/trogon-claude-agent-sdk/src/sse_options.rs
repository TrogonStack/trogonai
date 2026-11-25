use std::sync::Arc;

use indexmap::IndexMap;

use crate::SseDeliveryUpdate;

/// TypeScript `onCatchUpTruncated`.
pub type CatchUpTruncatedHandler = Arc<dyn Fn() + Send + Sync>;
/// TypeScript `onDeliveryUpdate`.
pub type DeliveryUpdateHandler = Arc<dyn Fn(SseDeliveryUpdate) + Send + Sync>;

/// TypeScript `SSEOptions`.
#[derive(Clone)]
pub struct SseOptions {
    /// The URL of the event stream, read with GET.
    pub stream_url: String,
    /// The URL written lines are POSTed to.
    pub send_url: String,
    /// The `session_id` of every POST body.
    pub session_id: String,
    /// Headers for the GET and the POSTs. `Accept` and `Content-Type` are set by the transport and win over a
    /// header of the same spelling.
    pub headers: IndexMap<String, String>,
    /// Resume after this sequence number when it is a positive safe integer; anything else starts from the
    /// beginning.
    pub from_sequence_num: Option<f64>,
    pub on_catch_up_truncated: Option<CatchUpTruncatedHandler>,
    pub on_delivery_update: Option<DeliveryUpdateHandler>,
}

impl SseOptions {
    pub fn new(stream_url: impl Into<String>, send_url: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            stream_url: stream_url.into(),
            send_url: send_url.into(),
            session_id: session_id.into(),
            headers: IndexMap::new(),
            from_sequence_num: None,
            on_catch_up_truncated: None,
            on_delivery_update: None,
        }
    }
}

impl std::fmt::Debug for SseOptions {
    /// Neither the URLs, the session id nor the header values appear: they commonly carry credentials.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseOptions")
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("from_sequence_num", &self.from_sequence_num)
            .field("on_catch_up_truncated", &self.on_catch_up_truncated.is_some())
            .field("on_delivery_update", &self.on_delivery_update.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "sse_options_tests.rs"]
mod tests;
