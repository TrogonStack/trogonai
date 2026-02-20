use serde::{Deserialize, Serialize};

// ── Primitives ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackFile {
    pub id: Option<String>,
    pub name: Option<String>,
    pub mimetype: Option<String>,
    pub url_private: Option<String>,
    pub url_private_download: Option<String>,
    pub size: Option<u64>,
    /// Decoded text content downloaded by slack-bot for text-like files.
    /// `None` for binary files, files exceeding the size limit, or when
    /// the download failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackAttachment {
    pub fallback: Option<String>,
    pub text: Option<String>,
    pub pretext: Option<String>,
    pub author_name: Option<String>,
    pub from_url: Option<String>,
    pub image_url: Option<String>,
    pub thumb_url: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub ts: Option<String>,
    #[serde(default)]
    pub files: Vec<SlackFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionType {
    Direct,
    Channel,
    Group,
}

// ── Inbound messages ─────────────────────────────────────────────────────────

/// A user (or app_mention) message flowing from Slack → NATS → agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackInboundMessage {
    pub channel: String,
    pub user: String,
    pub text: String,
    pub ts: String,
    pub event_ts: Option<String>,
    pub thread_ts: Option<String>,
    pub parent_user_id: Option<String>,
    pub session_type: SessionType,
    /// Set to "app_mention" when the message was an @-mention.
    pub source: Option<String>,
    /// Routing key for the agent to select the conversation context.
    ///
    /// Format (mirrors OpenClaw session naming):
    /// - DM:     `"slack:dm:<userId>"`
    /// - Channel:`"slack:channel:<channelId>"`
    /// - Thread: `"slack:channel:<channelId>:thread:<threadTs>"`
    /// - Group:  `"slack:group:<channelId>"` (with optional `:thread:` suffix)
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default)]
    pub files: Vec<SlackFile>,
    #[serde(default)]
    pub attachments: Vec<SlackAttachment>,
}

/// A message that was edited. Carries both old and new text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackMessageChangedEvent {
    pub channel: String,
    /// Timestamp of the edited message.
    pub ts: String,
    /// Timestamp of the `message_changed` wrapper event.
    pub event_ts: Option<String>,
    pub previous_text: Option<String>,
    pub new_text: Option<String>,
    pub thread_ts: Option<String>,
    pub user: Option<String>,
}

/// A message that was deleted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackMessageDeletedEvent {
    pub channel: String,
    /// Timestamp of the deleted message.
    pub deleted_ts: String,
    /// Timestamp of the `message_deleted` wrapper event.
    pub event_ts: Option<String>,
    pub thread_ts: Option<String>,
}

/// A thread reply that was also broadcast to the channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackThreadBroadcastEvent {
    pub channel: String,
    pub user: String,
    pub text: String,
    pub ts: String,
    pub thread_ts: String,
    pub event_ts: Option<String>,
}

// ── Reactions ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackReactionEvent {
    /// Emoji name without colons, e.g. `"thumbsup"`.
    pub reaction: String,
    /// User who reacted.
    pub user: String,
    /// Channel where the reacted-to message lives.
    pub channel: Option<String>,
    /// Timestamp of the reacted-to message.
    pub item_ts: Option<String>,
    /// Author of the reacted-to message.
    pub item_user: Option<String>,
    pub event_ts: String,
    /// `true` = reaction_added, `false` = reaction_removed.
    pub added: bool,
}

// ── Members ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackMemberEvent {
    pub user: String,
    pub channel: String,
    pub channel_type: Option<String>,
    pub team: Option<String>,
    pub inviter: Option<String>,
    /// `true` = joined, `false` = left.
    pub joined: bool,
}

// ── Channels ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelEventKind {
    Created,
    Deleted,
    Renamed,
    Archived,
    Unarchived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackChannelEvent {
    pub kind: ChannelEventKind,
    pub channel_id: String,
    pub channel_name: Option<String>,
    pub user: Option<String>,
}

// ── Slash commands ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSlashCommandEvent {
    pub command: String,
    pub text: Option<String>,
    pub user_id: String,
    pub channel_id: String,
    pub team_id: Option<String>,
    pub response_url: String,
    pub trigger_id: Option<String>,
}

// ── Outbound messages ────────────────────────────────────────────────────────

/// A response flowing from agent → NATS → slack-bot → Slack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackOutboundMessage {
    pub channel: String,
    pub text: String,
    pub thread_ts: Option<String>,
    /// Slack Block Kit blocks as raw JSON. When present, `text` is used as
    /// fallback for notifications only.
    pub blocks: Option<serde_json::Value>,
    /// URL of a file/image to upload alongside the message.
    pub media_url: Option<String>,
    /// Custom display name for this message (requires `chat:write.customize`).
    pub username: Option<String>,
    /// Custom icon URL (requires `chat:write.customize`).
    pub icon_url: Option<String>,
}

// ── Streaming outbound ───────────────────────────────────────────────────────

/// Request to start a new streaming message. The bot posts an initial message
/// to Slack and returns the message `ts` via NATS reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackStreamStartRequest {
    pub channel: String,
    pub thread_ts: Option<String>,
    /// Initial placeholder text shown before content streams in.
    pub initial_text: Option<String>,
}

/// NATS reply to `SlackStreamStartRequest` — carries the Slack message `ts`
/// needed for subsequent append/stop calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackStreamStartResponse {
    pub channel: String,
    pub ts: String,
}

/// Appends (replaces) the full accumulated text of an in-flight stream.
/// The bot calls `chat.update` with this text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackStreamAppendMessage {
    pub channel: String,
    /// The `ts` returned by `SlackStreamStartResponse`.
    pub ts: String,
    /// Full accumulated text to display (not a diff).
    pub text: String,
}

/// Finalises a streaming message. The bot calls `chat.update` one last time
/// with `final_text`, optionally attaching Block Kit blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackStreamStopMessage {
    pub channel: String,
    pub ts: String,
    pub final_text: String,
    pub blocks: Option<serde_json::Value>,
}

// ── Block actions ─────────────────────────────────────────────────────────────

/// A Block Kit interactive element was triggered (button click, select, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackBlockActionEvent {
    /// ID of the triggered action element.
    pub action_id: String,
    pub block_id: Option<String>,
    pub user_id: String,
    pub channel_id: Option<String>,
    /// Timestamp of the message that contains the block (for subsequent updates).
    pub message_ts: Option<String>,
    /// Static value set on the action element (e.g. button value).
    pub value: Option<String>,
    /// Trigger ID — pass to `views.open` if a modal response is needed.
    pub trigger_id: Option<String>,
}

// ── Reaction actions ─────────────────────────────────────────────────────────

/// Instructs the bot to add or remove a reaction on a Slack message.
///
/// Used for ack reactions: the agent adds an emoji while processing
/// and removes it once the response is delivered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackReactionAction {
    pub channel: String,
    /// Timestamp of the message to react to.
    pub ts: String,
    /// Emoji shortcode without colons, e.g. `"eyes"`.
    pub reaction: String,
    /// `true` = add the reaction, `false` = remove it.
    pub add: bool,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_message_roundtrip() {
        let msg = SlackInboundMessage {
            channel: "C123".into(),
            user: "U456".into(),
            text: "hello".into(),
            ts: "1234567890.123".into(),
            event_ts: None,
            thread_ts: Some("1234567890.000".into()),
            parent_user_id: None,
            session_type: SessionType::Channel,
            source: None,
            session_key: Some("slack:channel:C123:thread:1234567890.000".into()),
            files: vec![],
            attachments: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackInboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
        assert_eq!(decoded.text, "hello");
    }

    #[test]
    fn inbound_message_backward_compat_no_files() {
        // Old messages without files/attachments fields should still deserialise.
        let json =
            r#"{"channel":"C1","user":"U1","text":"hi","ts":"1.2","session_type":"Channel"}"#;
        let msg: SlackInboundMessage = serde_json::from_str(json).unwrap();
        assert!(msg.files.is_empty());
        assert!(msg.attachments.is_empty());
    }

    #[test]
    fn outbound_message_roundtrip() {
        let msg = SlackOutboundMessage {
            channel: "C123".into(),
            text: "world".into(),
            thread_ts: None,
            blocks: None,
            media_url: None,
            username: None,
            icon_url: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackOutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
    }

    #[test]
    fn outbound_with_blocks_roundtrip() {
        let msg = SlackOutboundMessage {
            channel: "C1".into(),
            text: "fallback".into(),
            thread_ts: None,
            blocks: Some(
                serde_json::json!([{"type":"section","text":{"type":"mrkdwn","text":"*hi*"}}]),
            ),
            media_url: None,
            username: Some("MyBot".into()),
            icon_url: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackOutboundMessage = serde_json::from_str(&json).unwrap();
        assert!(decoded.blocks.is_some());
        assert_eq!(decoded.username.unwrap(), "MyBot");
    }

    #[test]
    fn reaction_event_roundtrip() {
        let ev = SlackReactionEvent {
            reaction: "thumbsup".into(),
            user: "U1".into(),
            channel: Some("C1".into()),
            item_ts: Some("1.2".into()),
            item_user: None,
            event_ts: "1.3".into(),
            added: true,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackReactionEvent = serde_json::from_str(&json).unwrap();
        assert!(decoded.added);
        assert_eq!(decoded.reaction, "thumbsup");
    }

    #[test]
    fn stream_start_roundtrip() {
        let req = SlackStreamStartRequest {
            channel: "C1".into(),
            thread_ts: None,
            initial_text: Some("Thinking…".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let _: SlackStreamStartRequest = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn session_type_variants_serialize() {
        for v in [
            SessionType::Direct,
            SessionType::Channel,
            SessionType::Group,
        ] {
            let j = serde_json::to_string(&v).unwrap();
            let _: SessionType = serde_json::from_str(&j).unwrap();
        }
    }

    #[test]
    fn inbound_message_session_key_backward_compat() {
        // Old messages without session_key should deserialise with None.
        let json =
            r#"{"channel":"C1","user":"U1","text":"hi","ts":"1.2","session_type":"Channel"}"#;
        let msg: SlackInboundMessage = serde_json::from_str(json).unwrap();
        assert!(msg.session_key.is_none());
    }

    #[test]
    fn reaction_action_roundtrip() {
        let action = SlackReactionAction {
            channel: "C1".into(),
            ts: "1234567890.123".into(),
            reaction: "eyes".into(),
            add: true,
        };
        let json = serde_json::to_string(&action).unwrap();
        let decoded: SlackReactionAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.reaction, "eyes");
        assert!(decoded.add);
    }
}
