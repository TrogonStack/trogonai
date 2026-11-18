use std::fmt;

use trogon_nats::NatsToken;

use crate::SessionError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    namespace: NatsToken,
    session_id: String,
}

impl SessionKey {
    pub fn new(namespace: NatsToken, session_id: impl Into<String>) -> Result<Self, SessionError> {
        let session_id = session_id.into();
        if session_id.is_empty()
            || session_id.len() > 128
            || !session_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(SessionError::InvalidKey);
        }
        Ok(Self { namespace, session_id })
    }

    pub fn namespace(&self) -> &NatsToken {
        &self.namespace
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn subject(&self) -> String {
        format!("{}.session.sessions.{}", self.namespace, self.session_id)
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.subject())
    }
}
