//! Session ID generation for Discord conversations

use crate::types::ChannelType;

/// Generate a session ID for a Discord conversation.
///
/// - DM channels: `dc-dm-{channel_id}`
/// - Guild channels: `dc-guild-{guild_id}-{channel_id}`
pub fn session_id(
    channel_type: &ChannelType,
    channel_id: u64,
    guild_id: Option<u64>,
    _user_id: Option<u64>,
) -> String {
    match channel_type {
        ChannelType::Dm | ChannelType::GroupDm => {
            format!("dc-dm-{}", channel_id)
        }
        _ => {
            if let Some(gid) = guild_id {
                format!("dc-guild-{}-{}", gid, channel_id)
            } else {
                format!("dc-channel-{}", channel_id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dm_session_id() {
        let sid = session_id(&ChannelType::Dm, 123, None, Some(456));
        assert_eq!(sid, "dc-dm-123");
    }

    #[test]
    fn test_guild_session_id() {
        let sid = session_id(&ChannelType::GuildText, 100, Some(200), None);
        assert_eq!(sid, "dc-guild-200-100");
    }

    #[test]
    fn test_guild_text_without_guild_id() {
        let sid = session_id(&ChannelType::GuildText, 100, None, None);
        assert_eq!(sid, "dc-channel-100");
    }

    #[test]
    fn test_group_dm_session_id() {
        let sid = session_id(&ChannelType::GroupDm, 777, None, None);
        assert_eq!(sid, "dc-dm-777");
    }

    #[test]
    fn test_guild_voice_session_id() {
        let sid = session_id(&ChannelType::GuildVoice, 300, Some(200), None);
        assert_eq!(sid, "dc-guild-200-300");
    }
}
