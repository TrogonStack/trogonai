use super::stream_position::StreamPosition;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendStreamResponse {
    /// The stream high-watermark after the append completed.
    ///
    /// This is the value to store in projections, snapshots, realtime messages,
    /// and later `StreamState::At` preconditions. It is not a "next expected
    /// version" and it is not safe to perform arithmetic on it.
    pub stream_position: StreamPosition,
}
