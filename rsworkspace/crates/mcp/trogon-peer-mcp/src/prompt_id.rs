use trogon_peer::{NativeSessionId, PeerValueError};

pub(crate) struct PromptId(NativeSessionId);

impl PromptId {
    pub(crate) fn new(value: String) -> Result<Self, PeerValueError> {
        NativeSessionId::new(value).map(Self)
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
