/// Why an event stream could not be parsed.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SseError {
    /// An unterminated line or event grew past the buffer limit.
    #[error("event stream frame is too large")]
    FrameTooLarge,
    /// An event's data is not JSON.
    #[error("event stream frame is not JSON")]
    NotJson,
    /// An event's data is JSON but not an object.
    #[error("event stream frame is not a JSON object")]
    NotObject,
}
