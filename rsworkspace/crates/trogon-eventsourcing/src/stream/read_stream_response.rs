use super::stream_position::StreamPosition;
use crate::StreamEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct ReadStreamResponse {
    /// The latest comparable stream high-watermark observed by the store.
    ///
    /// This value is `None` when the stream has no current position. When it is
    /// present, callers may compare it with another `StreamPosition` from the
    /// same stream to answer freshness questions. Callers must not treat it as
    /// a gapless revision or event count.
    pub current_position: Option<StreamPosition>,
    pub events: Vec<StreamEvent>,
}
