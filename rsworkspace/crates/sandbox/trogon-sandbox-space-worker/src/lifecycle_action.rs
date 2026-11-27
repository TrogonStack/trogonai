/// The lifecycle verbs SPACE exposes as `POST /v1/sandboxes/{id}:<verb>`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LifecycleAction {
    /// Freeze the sandbox in memory.
    Pause,
    /// Move the sandbox to durable storage.
    Suspend,
    /// Bring a paused or suspended sandbox back.
    Wake,
}

impl LifecycleAction {
    /// The verb in the request path and in messages.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Suspend => "suspend",
            Self::Wake => "wake",
        }
    }
}
