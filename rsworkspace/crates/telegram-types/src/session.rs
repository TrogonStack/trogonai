//! Session ID generation and management

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::chat::{Chat, ChatType};

/// Session identifier
///
/// Format: `tg-{chat_type}-{chat_id}[-{user_id}]`
///
/// Examples:
/// - Private chat: `tg-private-123456789`
/// - Group: `tg-group-987654321`
/// - User in group (isolated): `tg-group-987654321-123456789`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Create a session ID for a private chat
    pub fn for_private_chat(chat_id: i64) -> Self {
        Self(format!("tg-private-{}", chat_id))
    }

    /// Create a session ID for a group chat
    pub fn for_group_chat(chat_id: i64) -> Self {
        Self(format!("tg-group-{}", chat_id))
    }

    /// Create a session ID for a supergroup chat
    pub fn for_supergroup_chat(chat_id: i64) -> Self {
        Self(format!("tg-supergroup-{}", chat_id))
    }

    /// Create a session ID for a channel
    pub fn for_channel(chat_id: i64) -> Self {
        Self(format!("tg-channel-{}", chat_id))
    }

    /// Create a session ID for a user in a group (isolated context)
    pub fn for_user_in_group(chat_id: i64, user_id: i64) -> Self {
        Self(format!("tg-group-{}-{}", chat_id, user_id))
    }

    /// Create a session ID from a chat
    pub fn from_chat(chat: &Chat) -> Self {
        match chat.chat_type {
            ChatType::Private => Self::for_private_chat(chat.id),
            ChatType::Group => Self::for_group_chat(chat.id),
            ChatType::Supergroup => Self::for_supergroup_chat(chat.id),
            ChatType::Channel => Self::for_channel(chat.id),
        }
    }

    /// Create a session ID from a chat with optional user isolation
    pub fn from_chat_with_user(chat: &Chat, user_id: Option<i64>, isolate_in_groups: bool) -> Self {
        match chat.chat_type {
            ChatType::Private => Self::for_private_chat(chat.id),
            ChatType::Group | ChatType::Supergroup => {
                if isolate_in_groups && user_id.is_some() {
                    Self::for_user_in_group(chat.id, user_id.unwrap())
                } else {
                    Self::from_chat(chat)
                }
            }
            ChatType::Channel => Self::for_channel(chat.id),
        }
    }

    /// Get the underlying string value
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Extract chat ID from session ID
    pub fn chat_id(&self) -> Option<i64> {
        let parts: Vec<&str> = self.0.split('-').collect();
        if parts.len() >= 3 {
            parts[2].parse().ok()
        } else {
            None
        }
    }

    /// Extract user ID from session ID (if isolated)
    pub fn user_id(&self) -> Option<i64> {
        let parts: Vec<&str> = self.0.split('-').collect();
        if parts.len() >= 4 {
            parts[3].parse().ok()
        } else {
            None
        }
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> String {
        id.0
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_private_session_id() {
        let id = SessionId::for_private_chat(123456789);
        assert_eq!(id.as_str(), "tg-private-123456789");
        assert_eq!(id.chat_id(), Some(123456789));
        assert_eq!(id.user_id(), None);
    }

    #[test]
    fn test_group_session_id() {
        let id = SessionId::for_group_chat(987654321);
        assert_eq!(id.as_str(), "tg-group-987654321");
        assert_eq!(id.chat_id(), Some(987654321));
        assert_eq!(id.user_id(), None);
    }

    #[test]
    fn test_user_in_group_session_id() {
        let id = SessionId::for_user_in_group(987654321, 123456789);
        assert_eq!(id.as_str(), "tg-group-987654321-123456789");
        assert_eq!(id.chat_id(), Some(987654321));
        assert_eq!(id.user_id(), Some(123456789));
    }
}
