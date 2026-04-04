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
    /// Base64-encoded bytes for image files (JPEG/PNG/GIF/WebP).
    /// Populated by the bot when the file is within the size limit for Claude vision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base64_content: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Resolved display name of the user (populated by slack-bot via users.info).
    /// Absent for old messages or when the lookup fails.
    #[serde(default)]
    pub display_name: Option<String>,
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
    /// Custom icon emoji (requires `chat:write.customize`). E.g. ":robot_face:".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_emoji: Option<String>,
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

// ── App Home ──────────────────────────────────────────────────────────────────

/// Emitted when a user opens the App Home tab in Slack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackAppHomeOpenedEvent {
    pub user: String,
    /// "home" or "messages"
    pub tab: String,
    /// The view currently displayed in the App Home, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_id: Option<String>,
}

// ── View submissions ──────────────────────────────────────────────────────────

/// A modal view was submitted by a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackViewSubmissionEvent {
    pub user_id: String,
    /// Trigger ID — may be used to open a follow-up modal.
    pub trigger_id: String,
    pub view_id: String,
    /// Callback ID set when the view was opened (identifies which modal this is).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_id: Option<String>,
    /// Block Kit state values: `{ "block_id": { "action_id": { ... } } }`.
    pub values: serde_json::Value,
}

// ── View closed ───────────────────────────────────────────────────────────────

/// A modal view was dismissed (X or Cancel) without submitting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackViewClosedEvent {
    pub user_id: String,
    pub trigger_id: String,
    pub view_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_id: Option<String>,
    /// Block Kit state values at the time of closing.
    pub values: serde_json::Value,
}

// ── Pins ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PinEventKind {
    Added,
    Removed,
}

/// A message was pinned or unpinned in a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackPinEvent {
    pub kind: PinEventKind,
    pub channel: String,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_ts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    pub event_ts: String,
}

// ── View outbound ─────────────────────────────────────────────────────────────

/// Instructs the bot to open a modal via `views.open`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackViewOpenRequest {
    /// Trigger ID from the block action or slash command that initiated this flow.
    pub trigger_id: String,
    /// Full Block Kit view payload as JSON (see Slack `views.open` docs).
    pub view: serde_json::Value,
}

/// Instructs the bot to publish (update) the App Home view via `views.publish`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackViewPublishRequest {
    /// The Slack user whose App Home should be updated.
    pub user_id: String,
    /// Full Block Kit view payload as JSON (see Slack `views.publish` docs).
    pub view: serde_json::Value,
}

// ── Typing status ─────────────────────────────────────────────────────────────

/// Instructs the bot to set (or clear) the "is thinking…" status on an
/// assistant thread via `assistant.threads.setStatus`.
/// Requires `assistant:write` scope and "Agents and AI Apps" enabled in the app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSetStatusRequest {
    pub channel_id: String,
    pub thread_ts: String,
    /// Status text. `None` clears the status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Instructs the bot to delete a Slack message (`chat.delete`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackDeleteMessage {
    pub channel: String,
    /// Timestamp of the message to delete.
    pub ts: String,
}

/// Instructs the bot to update an existing Slack message (`chat.update`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUpdateMessage {
    pub channel: String,
    /// Timestamp of the message to update.
    pub ts: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,
}

/// Request to fetch messages from a Slack channel (`conversations.history`).
/// Uses Core NATS request/reply — send to `SLACK_OUTBOUND_READ_MESSAGES`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackReadMessagesRequest {
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
}

/// A single message entry from `conversations.history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackReadMessage {
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

/// NATS reply payload for `SlackReadMessagesRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackReadMessagesResponse {
    pub ok: bool,
    #[serde(default)]
    pub messages: Vec<SlackReadMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Instructs the bot to upload text content as a file to Slack.
/// Uses `files.getUploadURLExternal` + upload + `files.completeUploadExternal`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUploadRequest {
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    /// File name including extension, e.g. "report.md" or "code.rs".
    pub filename: String,
    /// Text content to upload.
    pub content: String,
    /// Optional display title shown in Slack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}


// ── New feature types ────────────────────────────────────────────────────────

/// Request to fetch thread replies via `conversations.replies`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackReadRepliesRequest {
    pub channel: String,
    /// The `thread_ts` of the parent message.
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
}

/// Response from `conversations.replies`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackReadRepliesResponse {
    pub ok: bool,
    #[serde(default)]
    pub messages: Vec<SlackReadMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A single prompt suggestion for `assistant.threads.setSuggestedPrompts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSuggestedPrompt {
    pub title: String,
    pub message: String,
}

/// Request to set suggested prompts on an assistant thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackSetSuggestedPromptsRequest {
    pub channel_id: String,
    pub thread_ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub prompts: Vec<SlackSuggestedPrompt>,
}

/// Request to send a proactive message (without an inbound trigger).
/// Provide either `channel` (channel/DM ID) OR `user_id` (bot will open a DM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackProactiveMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// Custom icon emoji (requires `chat:write.customize`). E.g. ":robot_face:".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_emoji: Option<String>,
}

/// Request to post an ephemeral message (visible only to `user`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackEphemeralMessage {
    pub channel: String,
    pub user: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<String>,
}

/// Request to delete an uploaded file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackDeleteFile {
    pub file_id: String,
}

// ── Link sharing ─────────────────────────────────────────────────────────────

/// A `link_shared` event — one or more URLs were posted in a message.
/// The agent can respond with a `SlackUnfurlRequest` to attach rich previews.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackLinkSharedEvent {
    pub channel: String,
    /// Timestamp of the message that contained the links.
    pub message_ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_ts: Option<String>,
    /// The list of URLs found in the message.
    pub links: Vec<String>,
}

/// Instructs the bot to call `chat.unfurl` with the provided payloads.
///
/// `unfurls` maps each URL (from the original `link_shared` event) to its
/// preview payload.  Any URL not present in the map is left unattached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUnfurlRequest {
    pub channel: String,
    /// Timestamp of the message that contained the links.
    pub ts: String,
    /// URL → unfurl payload mapping sent verbatim to `chat.unfurl`.
    pub unfurls: std::collections::HashMap<String, SlackUnfurlPayload>,
}

/// Rich preview content for a single URL passed to `chat.unfurl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUnfurlPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

// ── Single user lookup ────────────────────────────────────────────────────────

/// Request to fetch a single user's profile via `users.info`.
/// Core NATS request/reply — send to `SLACK_OUTBOUND_GET_USER`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackGetUserRequest {
    pub user_id: String,
}

/// Response from `users.info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackGetUserResponse {
    pub ok: bool,
    /// The resolved user profile.  `None` when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<SlackListUsersUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Emoji list ────────────────────────────────────────────────────────────────

/// Request to fetch the workspace custom emoji map via `emoji.list`.
/// Core NATS request/reply — send to `SLACK_OUTBOUND_GET_EMOJI`.
/// No request fields are required; the bot returns all custom emoji.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackGetEmojiRequest {}

/// Response from `emoji.list`.
/// `emoji` maps shortcode names to their image URL (or an `alias:name` value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackGetEmojiResponse {
    pub ok: bool,
    #[serde(default)]
    pub emoji: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Slash command response_url ────────────────────────────────────────────────

/// Instructs the bot to POST a delayed response to a Slack `response_url` webhook.
///
/// Slash command response_url webhooks accept up to 5 messages within 30 minutes.
/// `ephemeral` controls the `response_type` field:
/// - `true`  → `"ephemeral"` (only visible to the invoking user)
/// - `false` → `"in_channel"` (visible to everyone in the channel)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackResponseUrlMessage {
    /// The Slack-provided `response_url` from the original slash command event.
    pub response_url: String,
    pub text: String,
    #[serde(default)]
    pub ephemeral: bool,
    /// Optional Block Kit blocks JSON string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<String>,
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
            display_name: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackInboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
        assert_eq!(decoded.text, "hello");
    }

    #[test]
    fn inbound_message_display_name_backward_compat() {
        // Old messages without display_name should deserialise with None.
        let json = r#"{"channel":"C1","user":"U1","text":"hi","ts":"1.2","session_type":"Channel"}"#;
        let msg: SlackInboundMessage = serde_json::from_str(json).unwrap();
        assert!(msg.display_name.is_none());
    }

    #[test]
    fn inbound_message_display_name_roundtrip() {
        let json = r#"{"channel":"C1","user":"U1","text":"hi","ts":"1.2","session_type":"Channel","display_name":"John Doe"}"#;
        let msg: SlackInboundMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.display_name.as_deref(), Some("John Doe"));
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
            icon_emoji: Some(":robot_face:".to_string()),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackOutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
        assert_eq!(decoded.icon_emoji.as_deref(), Some(":robot_face:"));
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
            icon_emoji: None,
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

    #[test]
    fn slack_file_roundtrip() {
        let file = SlackFile {
            id: Some("F123".into()),
            name: Some("report.txt".into()),
            mimetype: Some("text/plain".into()),
            url_private: Some("https://files.slack.com/files-pri/T1/report.txt".into()),
            url_private_download: Some(
                "https://files.slack.com/files-pri/T1/download/report.txt".into(),
            ),
            size: Some(1024),
            content: Some("hello world".into()),
            base64_content: None,
        };
        let json = serde_json::to_string(&file).unwrap();
        let decoded: SlackFile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id.as_deref(), Some("F123"));
        assert_eq!(decoded.content.as_deref(), Some("hello world"));
    }

    #[test]
    fn slack_file_without_content_roundtrip() {
        let file = SlackFile {
            id: Some("F456".into()),
            name: Some("image.png".into()),
            mimetype: Some("image/png".into()),
            url_private: None,
            url_private_download: None,
            size: Some(4096),
            content: None,
            base64_content: None,
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(!json.contains("content"));
        let decoded: SlackFile = serde_json::from_str(&json).unwrap();
        assert!(decoded.content.is_none());
    }

    #[test]
    fn slack_attachment_roundtrip() {
        let attachment = SlackAttachment {
            fallback: Some("A link".into()),
            text: Some("Attachment text".into()),
            pretext: Some("Pretext".into()),
            author_name: Some("Jane Doe".into()),
            from_url: Some("https://example.com".into()),
            image_url: Some("https://example.com/img.png".into()),
            thumb_url: Some("https://example.com/thumb.png".into()),
            channel_id: Some("C1".into()),
            channel_name: Some("general".into()),
            ts: Some("1234567890.000".into()),
            files: vec![SlackFile {
                id: Some("F789".into()),
                name: Some("doc.txt".into()),
                mimetype: Some("text/plain".into()),
                url_private: None,
                url_private_download: None,
                size: Some(512),
                content: None,
                base64_content: None,
            }],
        };
        let json = serde_json::to_string(&attachment).unwrap();
        let decoded: SlackAttachment = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.fallback.as_deref(), Some("A link"));
        assert_eq!(decoded.files.len(), 1);
        assert_eq!(decoded.files[0].id.as_deref(), Some("F789"));
    }

    #[test]
    fn slack_message_changed_event_roundtrip() {
        let ev = SlackMessageChangedEvent {
            channel: "C1".into(),
            ts: "1234567890.100".into(),
            event_ts: Some("1234567890.200".into()),
            previous_text: Some("old text".into()),
            new_text: Some("new text".into()),
            thread_ts: Some("1234567890.000".into()),
            user: Some("U1".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackMessageChangedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C1");
        assert_eq!(decoded.new_text.as_deref(), Some("new text"));
        assert_eq!(decoded.previous_text.as_deref(), Some("old text"));
    }

    #[test]
    fn slack_message_deleted_event_roundtrip() {
        let ev = SlackMessageDeletedEvent {
            channel: "C2".into(),
            deleted_ts: "1234567890.111".into(),
            event_ts: Some("1234567890.222".into()),
            thread_ts: Some("1234567890.000".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackMessageDeletedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C2");
        assert_eq!(decoded.deleted_ts, "1234567890.111");
    }

    #[test]
    fn slack_thread_broadcast_event_roundtrip() {
        let ev = SlackThreadBroadcastEvent {
            channel: "C3".into(),
            user: "U2".into(),
            text: "broadcasted reply".into(),
            ts: "1234567890.333".into(),
            thread_ts: "1234567890.000".into(),
            event_ts: Some("1234567890.444".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackThreadBroadcastEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C3");
        assert_eq!(decoded.text, "broadcasted reply");
        assert_eq!(decoded.thread_ts, "1234567890.000");
    }

    #[test]
    fn slack_member_event_roundtrip() {
        let ev = SlackMemberEvent {
            user: "U3".into(),
            channel: "C4".into(),
            channel_type: Some("channel".into()),
            team: Some("T1".into()),
            inviter: Some("U0".into()),
            joined: true,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackMemberEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user, "U3");
        assert!(decoded.joined);
        assert_eq!(decoded.inviter.as_deref(), Some("U0"));
    }

    #[test]
    fn channel_event_kind_all_variants() {
        let variants = [
            ChannelEventKind::Created,
            ChannelEventKind::Deleted,
            ChannelEventKind::Renamed,
            ChannelEventKind::Archived,
            ChannelEventKind::Unarchived,
        ];
        for kind in variants {
            let json = serde_json::to_string(&kind).unwrap();
            let _: ChannelEventKind = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn slack_channel_event_roundtrip() {
        let ev = SlackChannelEvent {
            kind: ChannelEventKind::Renamed,
            channel_id: "C5".into(),
            channel_name: Some("renamed-channel".into()),
            user: Some("U4".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackChannelEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel_id, "C5");
        assert_eq!(decoded.channel_name.as_deref(), Some("renamed-channel"));
    }

    #[test]
    fn slack_slash_command_event_roundtrip() {
        let ev = SlackSlashCommandEvent {
            command: "/deploy".into(),
            text: Some("production".into()),
            user_id: "U5".into(),
            channel_id: "C6".into(),
            team_id: Some("T2".into()),
            response_url: "https://hooks.slack.com/commands/T2/123/abc".into(),
            trigger_id: Some("1234567.890".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackSlashCommandEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.command, "/deploy");
        assert_eq!(decoded.text.as_deref(), Some("production"));
        assert_eq!(decoded.user_id, "U5");
    }

    #[test]
    fn slack_block_action_event_roundtrip() {
        let ev = SlackBlockActionEvent {
            action_id: "approve_button".into(),
            block_id: Some("approval_block".into()),
            user_id: "U6".into(),
            channel_id: Some("C7".into()),
            message_ts: Some("1234567890.555".into()),
            value: Some("approved".into()),
            trigger_id: Some("9876543.210".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackBlockActionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.action_id, "approve_button");
        assert_eq!(decoded.user_id, "U6");
        assert_eq!(decoded.value.as_deref(), Some("approved"));
    }

    #[test]
    fn slack_app_home_opened_event_roundtrip() {
        let ev = SlackAppHomeOpenedEvent {
            user: "U7".into(),
            tab: "home".into(),
            view_id: Some("V123".into()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackAppHomeOpenedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user, "U7");
        assert_eq!(decoded.tab, "home");
        assert_eq!(decoded.view_id.as_deref(), Some("V123"));
    }

    #[test]
    fn app_home_opened_without_view_id() {
        let json = r#"{"user":"U8","tab":"messages"}"#;
        let decoded: SlackAppHomeOpenedEvent = serde_json::from_str(json).unwrap();
        assert_eq!(decoded.user, "U8");
        assert_eq!(decoded.tab, "messages");
        assert!(decoded.view_id.is_none());
    }

    #[test]
    fn slack_view_submission_event_roundtrip() {
        let ev = SlackViewSubmissionEvent {
            user_id: "U9".into(),
            trigger_id: "1111111.222".into(),
            view_id: "V456".into(),
            callback_id: Some("my_modal".into()),
            values: serde_json::json!({"block_1": {"input_1": {"value": "some text"}}}),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: SlackViewSubmissionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user_id, "U9");
        assert_eq!(decoded.view_id, "V456");
        assert_eq!(decoded.callback_id.as_deref(), Some("my_modal"));
    }

    #[test]
    fn view_submission_without_callback_id() {
        let json = r#"{"user_id":"U10","trigger_id":"3333.444","view_id":"V789","values":{}}"#;
        let decoded: SlackViewSubmissionEvent = serde_json::from_str(json).unwrap();
        assert_eq!(decoded.user_id, "U10");
        assert_eq!(decoded.view_id, "V789");
        assert!(decoded.callback_id.is_none());
    }

    #[test]
    fn slack_view_open_request_roundtrip() {
        let req = SlackViewOpenRequest {
            trigger_id: "5555555.666".into(),
            view: serde_json::json!({"type": "modal", "title": {"type": "plain_text", "text": "My Modal"}, "blocks": []}),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackViewOpenRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.trigger_id, "5555555.666");
        assert_eq!(decoded.view["type"], "modal");
    }

    #[test]
    fn slack_view_publish_request_roundtrip() {
        let req = SlackViewPublishRequest {
            user_id: "U11".into(),
            view: serde_json::json!({"type": "home", "blocks": []}),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackViewPublishRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user_id, "U11");
        assert_eq!(decoded.view["type"], "home");
    }

    #[test]
    fn slack_stream_append_message_roundtrip() {
        let msg = SlackStreamAppendMessage {
            channel: "C8".into(),
            ts: "1234567890.777".into(),
            text: "partial response so far...".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackStreamAppendMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C8");
        assert_eq!(decoded.ts, "1234567890.777");
        assert_eq!(decoded.text, "partial response so far...");
    }

    #[test]
    fn slack_stream_stop_message_roundtrip() {
        let msg = SlackStreamStopMessage {
            channel: "C9".into(),
            ts: "1234567890.888".into(),
            final_text: "complete response".into(),
            blocks: Some(
                serde_json::json!([{"type": "section", "text": {"type": "mrkdwn", "text": "complete response"}}]),
            ),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackStreamStopMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C9");
        assert_eq!(decoded.final_text, "complete response");
        assert!(decoded.blocks.is_some());
    }

    #[test]
    fn slack_delete_message_roundtrip() {
        let msg = SlackDeleteMessage {
            channel: "C123".into(),
            ts: "1234567890.001".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackDeleteMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
        assert_eq!(decoded.ts, "1234567890.001");
    }

    #[test]
    fn slack_update_message_roundtrip() {
        let msg = SlackUpdateMessage {
            channel: "C456".into(),
            ts: "1234567890.002".into(),
            text: "updated text".into(),
            blocks: Some(serde_json::json!([{"type": "section"}])),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackUpdateMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C456");
        assert_eq!(decoded.ts, "1234567890.002");
        assert_eq!(decoded.text, "updated text");
        assert!(decoded.blocks.is_some());
    }

    #[test]
    fn slack_file_base64_content_backward_compat() {
        let json = r#"{"id":"F1","name":"photo.png","mimetype":"image/png"}"#;
        let f: SlackFile = serde_json::from_str(json).unwrap();
        assert!(f.base64_content.is_none());
    }
    #[test]
    fn slack_read_messages_request_roundtrip() {
        let req = SlackReadMessagesRequest {
            channel: "C123456".into(),
            limit: Some(50),
            oldest: Some("1609459200.000000".into()),
            latest: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackReadMessagesRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123456");
        assert_eq!(decoded.limit, Some(50));
        assert_eq!(decoded.oldest, Some("1609459200.000000".into()));
        assert!(decoded.latest.is_none());
    }

    #[test]
    fn slack_read_message_roundtrip() {
        let msg = SlackReadMessage {
            ts: "1609459200.000001".into(),
            user: Some("U123".into()),
            text: Some("Hello world".into()),
            bot_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackReadMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.ts, "1609459200.000001");
        assert_eq!(decoded.user, Some("U123".into()));
        assert_eq!(decoded.text, Some("Hello world".into()));
        assert!(decoded.bot_id.is_none());
    }

    #[test]
    fn slack_read_messages_response_roundtrip() {
        let resp = SlackReadMessagesResponse {
            ok: true,
            messages: vec![
                SlackReadMessage {
                    ts: "1609459200.000001".into(),
                    user: Some("U123".into()),
                    text: Some("Hello".into()),
                    bot_id: None,
                },
            ],
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: SlackReadMessagesResponse = serde_json::from_str(&json).unwrap();
        assert!(decoded.ok);
        assert_eq!(decoded.messages.len(), 1);
        assert_eq!(decoded.messages[0].ts, "1609459200.000001");
        assert!(decoded.error.is_none());
    }


    #[test]
    fn slack_upload_request_roundtrip() {
        let req = SlackUploadRequest {
            channel: "C123".into(),
            thread_ts: Some("1609459200.000001".into()),
            filename: "report.md".into(),
            content: "# Hello\nWorld".into(),
            title: Some("My Report".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackUploadRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
        assert_eq!(decoded.thread_ts.as_deref(), Some("1609459200.000001"));
        assert_eq!(decoded.filename, "report.md");
        assert_eq!(decoded.content, "# Hello\nWorld");
        assert_eq!(decoded.title.as_deref(), Some("My Report"));
    }


    #[test]
    fn slack_read_replies_request_roundtrip() {
        let req = SlackReadRepliesRequest {
            channel: "C123".into(),
            ts: "1609459200.000001".into(),
            limit: Some(50),
            oldest: None,
            latest: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackReadRepliesRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C123");
        assert_eq!(decoded.ts, "1609459200.000001");
        assert_eq!(decoded.limit, Some(50));
    }

    #[test]
    fn slack_read_replies_response_roundtrip() {
        let resp = SlackReadRepliesResponse {
            ok: true,
            messages: vec![SlackReadMessage {
                ts: "1609459200.000002".into(),
                user: Some("U456".into()),
                text: Some("Reply".into()),
                bot_id: None,
            }],
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: SlackReadRepliesResponse = serde_json::from_str(&json).unwrap();
        assert!(decoded.ok);
        assert_eq!(decoded.messages.len(), 1);
        assert_eq!(decoded.messages[0].ts, "1609459200.000002");
    }

    #[test]
    fn slack_set_suggested_prompts_request_roundtrip() {
        let req = SlackSetSuggestedPromptsRequest {
            channel_id: "C789".into(),
            thread_ts: "1609459200.000003".into(),
            title: Some("What can I help with?".into()),
            prompts: vec![SlackSuggestedPrompt {
                title: "Summarise".into(),
                message: "Please summarise this thread.".into(),
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackSetSuggestedPromptsRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel_id, "C789");
        assert_eq!(decoded.prompts.len(), 1);
        assert_eq!(decoded.prompts[0].title, "Summarise");
    }

    #[test]
    fn slack_proactive_message_roundtrip() {
        let msg = SlackProactiveMessage {
            channel: None,
            user_id: Some("U001".into()),
            text: "Hello proactively!".into(),
            thread_ts: None,
            blocks: None,
            username: Some("MyBot".into()),
            icon_url: None,
            icon_emoji: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackProactiveMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user_id.as_deref(), Some("U001"));
        assert_eq!(decoded.text, "Hello proactively!");
        assert_eq!(decoded.username.as_deref(), Some("MyBot"));
    }

    #[test]
    fn slack_ephemeral_message_roundtrip() {
        let msg = SlackEphemeralMessage {
            channel: "C111".into(),
            user: "U222".into(),
            text: "Only you can see this.".into(),
            thread_ts: Some("1609459200.000005".into()),
            blocks: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SlackEphemeralMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.channel, "C111");
        assert_eq!(decoded.user, "U222");
        assert_eq!(decoded.thread_ts.as_deref(), Some("1609459200.000005"));
    }

    #[test]
    fn slack_delete_file_roundtrip() {
        let req = SlackDeleteFile {
            file_id: "F333XYZ".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackDeleteFile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.file_id, "F333XYZ");
    }


}

// ── users.list and conversations.list types ───────────────────────────────────

/// Request to list workspace users.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackListUsersRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A single user in the workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackListUsersUser {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub real_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub is_bot: bool,
    pub deleted: bool,
}

/// Response from users.list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackListUsersResponse {
    pub ok: bool,
    #[serde(default)]
    pub members: Vec<SlackListUsersUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request to list channels/conversations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackListConversationsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<String>,
}

/// A single conversation/channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackListConversationsChannel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub is_channel: bool,
    pub is_private: bool,
    pub is_archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_members: Option<u32>,
}

/// Response from conversations.list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackListConversationsResponse {
    pub ok: bool,
    #[serde(default)]
    pub channels: Vec<SlackListConversationsChannel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod list_tests {
    use super::*;

    #[test]
    fn slack_list_users_request_roundtrip() {
        let req = SlackListUsersRequest {
            limit: Some(100),
            cursor: Some("dXNlcjox".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackListUsersRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.limit, Some(100));
        assert_eq!(decoded.cursor.as_deref(), Some("dXNlcjox"));

        // None fields are skipped
        let req_no_cursor = SlackListUsersRequest { limit: Some(10), cursor: None };
        let json2 = serde_json::to_string(&req_no_cursor).unwrap();
        assert!(!json2.contains("cursor"));
    }

    #[test]
    fn slack_list_users_user_roundtrip() {
        let user = SlackListUsersUser {
            id: "U123".into(),
            name: "alice".into(),
            real_name: Some("Alice Smith".into()),
            display_name: Some("alice".into()),
            is_bot: false,
            deleted: false,
        };
        let json = serde_json::to_string(&user).unwrap();
        let decoded: SlackListUsersUser = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "U123");
        assert_eq!(decoded.name, "alice");
        assert_eq!(decoded.real_name.as_deref(), Some("Alice Smith"));
        assert_eq!(decoded.display_name.as_deref(), Some("alice"));
        assert!(!decoded.is_bot);
        assert!(!decoded.deleted);
    }

    #[test]
    fn slack_list_users_response_roundtrip() {
        let resp = SlackListUsersResponse {
            ok: true,
            members: vec![SlackListUsersUser {
                id: "U001".into(),
                name: "bob".into(),
                real_name: None,
                display_name: None,
                is_bot: false,
                deleted: false,
            }],
            next_cursor: Some("abc123".into()),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: SlackListUsersResponse = serde_json::from_str(&json).unwrap();
        assert!(decoded.ok);
        assert_eq!(decoded.members.len(), 1);
        assert_eq!(decoded.members[0].id, "U001");
        assert_eq!(decoded.next_cursor.as_deref(), Some("abc123"));
        assert!(decoded.error.is_none());
    }

    #[test]
    fn slack_list_conversations_request_roundtrip() {
        let req = SlackListConversationsRequest {
            limit: Some(50),
            cursor: None,
            exclude_archived: Some(true),
            types: Some("public_channel,private_channel".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: SlackListConversationsRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.limit, Some(50));
        assert!(decoded.cursor.is_none());
        assert_eq!(decoded.exclude_archived, Some(true));
        assert_eq!(decoded.types.as_deref(), Some("public_channel,private_channel"));
    }

    #[test]
    fn slack_list_conversations_channel_roundtrip() {
        let ch = SlackListConversationsChannel {
            id: "C123".into(),
            name: Some("general".into()),
            is_channel: true,
            is_private: false,
            is_archived: false,
            num_members: Some(42),
        };
        let json = serde_json::to_string(&ch).unwrap();
        let decoded: SlackListConversationsChannel = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "C123");
        assert_eq!(decoded.name.as_deref(), Some("general"));
        assert!(decoded.is_channel);
        assert!(!decoded.is_private);
        assert!(!decoded.is_archived);
        assert_eq!(decoded.num_members, Some(42));
    }

    #[test]
    fn slack_list_conversations_response_roundtrip() {
        let resp = SlackListConversationsResponse {
            ok: true,
            channels: vec![SlackListConversationsChannel {
                id: "C999".into(),
                name: Some("random".into()),
                is_channel: true,
                is_private: false,
                is_archived: false,
                num_members: None,
            }],
            next_cursor: None,
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: SlackListConversationsResponse = serde_json::from_str(&json).unwrap();
        assert!(decoded.ok);
        assert_eq!(decoded.channels.len(), 1);
        assert_eq!(decoded.channels[0].id, "C999");
        assert!(decoded.next_cursor.is_none());
    }
}
