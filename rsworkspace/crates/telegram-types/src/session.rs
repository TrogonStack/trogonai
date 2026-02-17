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
                if isolate_in_groups {
                    if let Some(uid) = user_id {
                        return Self::for_user_in_group(chat.id, uid);
                    }
                }
                Self::from_chat(chat)
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

    fn private_chat() -> Chat {
        Chat {
            id: 123456789,
            chat_type: ChatType::Private,
            title: None,
            username: None,
        }
    }
    fn group_chat() -> Chat {
        Chat {
            id: 987654321,
            chat_type: ChatType::Group,
            title: None,
            username: None,
        }
    }
    fn supergroup_chat() -> Chat {
        Chat {
            id: 111222333,
            chat_type: ChatType::Supergroup,
            title: None,
            username: None,
        }
    }
    fn channel_chat() -> Chat {
        Chat {
            id: 444555666,
            chat_type: ChatType::Channel,
            title: None,
            username: None,
        }
    }

    // ── Constructor functions ─────────────────────────────────────────────────

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
    fn test_supergroup_session_id() {
        let id = SessionId::for_supergroup_chat(111222333);
        assert_eq!(id.as_str(), "tg-supergroup-111222333");
        assert_eq!(id.chat_id(), Some(111222333));
        assert_eq!(id.user_id(), None);
    }

    #[test]
    fn test_channel_session_id() {
        let id = SessionId::for_channel(444555666);
        assert_eq!(id.as_str(), "tg-channel-444555666");
        assert_eq!(id.chat_id(), Some(444555666));
        assert_eq!(id.user_id(), None);
    }

    #[test]
    fn test_user_in_group_session_id() {
        let id = SessionId::for_user_in_group(987654321, 123456789);
        assert_eq!(id.as_str(), "tg-group-987654321-123456789");
        assert_eq!(id.chat_id(), Some(987654321));
        assert_eq!(id.user_id(), Some(123456789));
    }

    // ── from_chat() ───────────────────────────────────────────────────────────

    #[test]
    fn test_from_chat_private() {
        let id = SessionId::from_chat(&private_chat());
        assert_eq!(id.as_str(), "tg-private-123456789");
    }

    #[test]
    fn test_from_chat_group() {
        let id = SessionId::from_chat(&group_chat());
        assert_eq!(id.as_str(), "tg-group-987654321");
    }

    #[test]
    fn test_from_chat_supergroup() {
        let id = SessionId::from_chat(&supergroup_chat());
        assert_eq!(id.as_str(), "tg-supergroup-111222333");
    }

    #[test]
    fn test_from_chat_channel() {
        let id = SessionId::from_chat(&channel_chat());
        assert_eq!(id.as_str(), "tg-channel-444555666");
    }

    // ── from_chat_with_user() ─────────────────────────────────────────────────

    #[test]
    fn test_from_chat_with_user_private_ignores_isolation() {
        // Private chats never use user isolation — user_id and isolate flag ignored
        let id = SessionId::from_chat_with_user(&private_chat(), Some(42), true);
        assert_eq!(id.as_str(), "tg-private-123456789");
    }

    #[test]
    fn test_from_chat_with_user_group_isolated() {
        let id = SessionId::from_chat_with_user(&group_chat(), Some(42), true);
        assert_eq!(id.as_str(), "tg-group-987654321-42");
        assert_eq!(id.user_id(), Some(42));
    }

    #[test]
    fn test_from_chat_with_user_group_not_isolated() {
        let id = SessionId::from_chat_with_user(&group_chat(), Some(42), false);
        assert_eq!(id.as_str(), "tg-group-987654321");
        assert_eq!(id.user_id(), None);
    }

    #[test]
    fn test_from_chat_with_user_group_no_user_id() {
        // isolation=true but no user_id → falls back to shared session
        let id = SessionId::from_chat_with_user(&group_chat(), None, true);
        assert_eq!(id.as_str(), "tg-group-987654321");
    }

    #[test]
    fn test_from_chat_with_user_supergroup_isolated() {
        let id = SessionId::from_chat_with_user(&supergroup_chat(), Some(99), true);
        assert_eq!(id.as_str(), "tg-group-111222333-99");
    }

    #[test]
    fn test_from_chat_with_user_channel_ignores_isolation() {
        // Channels don't support user isolation
        let id = SessionId::from_chat_with_user(&channel_chat(), Some(99), true);
        assert_eq!(id.as_str(), "tg-channel-444555666");
    }

    // ── Trait implementations ─────────────────────────────────────────────────

    #[test]
    fn test_display() {
        let id = SessionId::for_private_chat(42);
        assert_eq!(id.to_string(), "tg-private-42");
    }

    #[test]
    fn test_from_string() {
        let id: SessionId = "tg-custom-session".to_string().into();
        assert_eq!(id.as_str(), "tg-custom-session");
    }

    #[test]
    fn test_as_ref_str() {
        let id = SessionId::for_private_chat(1);
        let s: &str = id.as_ref();
        assert_eq!(s, "tg-private-1");
    }

    #[test]
    fn test_into_string() {
        let id = SessionId::for_private_chat(1);
        let s: String = id.into();
        assert_eq!(s, "tg-private-1");
    }

    #[test]
    fn test_equality() {
        let a = SessionId::for_private_chat(123);
        let b = SessionId::for_private_chat(123);
        let c = SessionId::for_private_chat(456);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_hash_consistency() {
        use std::collections::HashMap;
        let mut map: HashMap<SessionId, &str> = HashMap::new();
        let id = SessionId::for_private_chat(42);
        map.insert(id.clone(), "value");
        assert_eq!(map[&id], "value");
    }

    // ── serde ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_serde_roundtrip() {
        let id = SessionId::for_user_in_group(100, 200);
        let json = serde_json::to_string(&id).unwrap();
        let back: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_serializes_as_plain_string() {
        let id = SessionId::for_private_chat(7);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"tg-private-7\"");
    }
}
