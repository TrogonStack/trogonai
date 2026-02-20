// ── Inbound (Slack → NATS → agent) ──────────────────────────────────────────

/// Regular messages and DMs from users.
pub const SLACK_INBOUND: &str = "slack.inbound.message";

/// reaction_added and reaction_removed events.
pub const SLACK_INBOUND_REACTION: &str = "slack.inbound.reaction";

/// message_changed (edited) sub-events.
pub const SLACK_INBOUND_MESSAGE_CHANGED: &str = "slack.inbound.message_changed";

/// message_deleted sub-events.
pub const SLACK_INBOUND_MESSAGE_DELETED: &str = "slack.inbound.message_deleted";

/// Thread replies broadcast to the channel.
pub const SLACK_INBOUND_THREAD_BROADCAST: &str = "slack.inbound.thread_broadcast";

/// member_joined_channel and member_left_channel events.
pub const SLACK_INBOUND_MEMBER: &str = "slack.inbound.member";

/// channel_created, channel_deleted, channel_rename, channel_archive events.
pub const SLACK_INBOUND_CHANNEL: &str = "slack.inbound.channel";

/// Slash command invocations.
pub const SLACK_INBOUND_SLASH_COMMAND: &str = "slack.inbound.slash_command";

/// Block Kit interactive component events (button clicks, selects, etc.).
pub const SLACK_INBOUND_BLOCK_ACTION: &str = "slack.inbound.block_action";

/// User opened the App Home tab.
pub const SLACK_INBOUND_APP_HOME: &str = "slack.inbound.app_home";

/// A modal view was submitted by a user.
pub const SLACK_INBOUND_VIEW_SUBMISSION: &str = "slack.inbound.view_submission";

/// A modal view was dismissed without submitting.
pub const SLACK_INBOUND_VIEW_CLOSED: &str = "slack.inbound.view_closed";

/// pin_added and pin_removed events.
pub const SLACK_INBOUND_PIN: &str = "slack.inbound.pin";

// ── Outbound (agent → NATS → slack-bot → Slack) ─────────────────────────────

/// Simple one-shot responses (`chat.postMessage`).
pub const SLACK_OUTBOUND: &str = "slack.outbound.message";

/// Request to post the initial placeholder of a streaming message.
/// Uses NATS request/reply — the bot responds with `SlackStreamStartResponse`
/// containing the Slack message `ts`.
pub const SLACK_OUTBOUND_STREAM_START: &str = "slack.outbound.stream.start";

/// Update the body of an in-flight streaming message (`chat.update`).
pub const SLACK_OUTBOUND_STREAM_APPEND: &str = "slack.outbound.stream.append";

/// Finalise a streaming message with the complete text (`chat.update`).
pub const SLACK_OUTBOUND_STREAM_STOP: &str = "slack.outbound.stream.stop";

/// Add or remove a reaction emoji on a Slack message (`reactions.add` / `reactions.remove`).
pub const SLACK_OUTBOUND_REACTION: &str = "slack.outbound.reaction";

/// Open a modal (`views.open`).
pub const SLACK_OUTBOUND_VIEW_OPEN: &str = "slack.outbound.view.open";

/// Publish (update) the App Home view (`views.publish`).
pub const SLACK_OUTBOUND_VIEW_PUBLISH: &str = "slack.outbound.view.publish";

/// Set or clear the "is thinking…" status on an assistant thread
/// (`assistant.threads.setStatus`). Requires `assistant:write` scope.
pub const SLACK_OUTBOUND_SET_STATUS: &str = "slack.outbound.set_status";

/// Delete a Slack message (`chat.delete`).
pub const SLACK_OUTBOUND_DELETE: &str = "slack.outbound.delete";

/// Update an existing Slack message (`chat.update`).
pub const SLACK_OUTBOUND_UPDATE: &str = "slack.outbound.update";

/// Request channel history (`conversations.history`). Core NATS request/reply.
pub const SLACK_OUTBOUND_READ_MESSAGES: &str = "slack.outbound.read_messages";

/// Upload text content as a Slack file. JetStream pub/sub.
pub const SLACK_OUTBOUND_UPLOAD: &str = "slack.outbound.upload";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_subjects_are_non_empty_and_distinct() {
        let subjects = [
            SLACK_INBOUND,
            SLACK_INBOUND_REACTION,
            SLACK_INBOUND_MESSAGE_CHANGED,
            SLACK_INBOUND_MESSAGE_DELETED,
            SLACK_INBOUND_THREAD_BROADCAST,
            SLACK_INBOUND_MEMBER,
            SLACK_INBOUND_CHANNEL,
            SLACK_INBOUND_SLASH_COMMAND,
            SLACK_INBOUND_BLOCK_ACTION,
            SLACK_INBOUND_APP_HOME,
            SLACK_INBOUND_VIEW_SUBMISSION,
            SLACK_INBOUND_VIEW_CLOSED,
            SLACK_INBOUND_PIN,
        ];
        for s in subjects {
            assert!(!s.is_empty());
        }
        let mut seen = std::collections::HashSet::new();
        for s in subjects {
            assert!(seen.insert(s), "duplicate subject: {s}");
        }
    }

    #[test]
    fn outbound_subjects_are_distinct() {
        let subjects = [
            SLACK_OUTBOUND,
            SLACK_OUTBOUND_STREAM_START,
            SLACK_OUTBOUND_STREAM_APPEND,
            SLACK_OUTBOUND_STREAM_STOP,
            SLACK_OUTBOUND_REACTION,
            SLACK_OUTBOUND_VIEW_OPEN,
            SLACK_OUTBOUND_VIEW_PUBLISH,
            SLACK_OUTBOUND_SET_STATUS,
            SLACK_OUTBOUND_DELETE,
            SLACK_OUTBOUND_UPDATE,
            SLACK_OUTBOUND_READ_MESSAGES,
            SLACK_OUTBOUND_UPLOAD,
        ];
        let mut seen = std::collections::HashSet::new();
        for s in subjects {
            assert!(seen.insert(s), "duplicate subject: {s}");
        }
    }
}
