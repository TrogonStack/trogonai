//! Integration tests for slack-nats using mock NATS infrastructure.
//!
//! These tests simulate the full Slack ↔ NATS message pipeline without a
//! real NATS server, using `MockNatsClient` and `AdvancedMockNatsClient`
//! from `trogon-nats`.
//!
//! Flow under test:
//!   Slack → slack-bot (publish_inbound) → NATS [mock] → slack-agent (publish_outbound)
//!                                                       → NATS [mock] → slack-bot

use slack_nats::publisher::{
    publish_app_home, publish_block_action, publish_channel, publish_delete_file,
    publish_delete_message, publish_ephemeral_message, publish_inbound, publish_member,
    publish_message_changed, publish_message_deleted, publish_outbound, publish_pin,
    publish_proactive_message, publish_reaction, publish_reaction_action, publish_set_status,
    publish_set_suggested_prompts, publish_slash_command, publish_stream_append,
    publish_stream_stop, publish_thread_broadcast, publish_update_message, publish_upload_request,
    publish_view_closed, publish_view_open, publish_view_publish, publish_view_submission,
    request_list_conversations, request_list_users, request_read_messages, request_read_replies,
};
use slack_types::events::{
    ChannelEventKind, PinEventKind, SessionType, SlackAppHomeOpenedEvent, SlackBlockActionEvent,
    SlackChannelEvent, SlackDeleteFile, SlackDeleteMessage, SlackEphemeralMessage,
    SlackInboundMessage, SlackListConversationsChannel, SlackListConversationsRequest,
    SlackListConversationsResponse, SlackListUsersRequest, SlackListUsersResponse,
    SlackListUsersUser, SlackMemberEvent, SlackMessageChangedEvent, SlackMessageDeletedEvent,
    SlackOutboundMessage, SlackPinEvent, SlackProactiveMessage, SlackReactionAction,
    SlackReactionEvent, SlackReadMessage, SlackReadMessagesRequest, SlackReadMessagesResponse,
    SlackReadRepliesRequest, SlackReadRepliesResponse, SlackSetStatusRequest,
    SlackSetSuggestedPromptsRequest, SlackSlashCommandEvent, SlackStreamAppendMessage,
    SlackStreamStopMessage, SlackSuggestedPrompt, SlackThreadBroadcastEvent, SlackUpdateMessage,
    SlackUploadRequest, SlackViewClosedEvent, SlackViewOpenRequest, SlackViewPublishRequest,
    SlackViewSubmissionEvent,
};
use slack_types::subjects::{
    SLACK_INBOUND, SLACK_INBOUND_APP_HOME, SLACK_INBOUND_BLOCK_ACTION, SLACK_INBOUND_CHANNEL,
    SLACK_INBOUND_MEMBER, SLACK_INBOUND_MESSAGE_CHANGED, SLACK_INBOUND_MESSAGE_DELETED,
    SLACK_INBOUND_PIN, SLACK_INBOUND_REACTION, SLACK_INBOUND_SLASH_COMMAND,
    SLACK_INBOUND_THREAD_BROADCAST, SLACK_INBOUND_VIEW_CLOSED, SLACK_INBOUND_VIEW_SUBMISSION,
    SLACK_OUTBOUND, SLACK_OUTBOUND_DELETE, SLACK_OUTBOUND_DELETE_FILE, SLACK_OUTBOUND_EPHEMERAL,
    SLACK_OUTBOUND_LIST_CONVERSATIONS, SLACK_OUTBOUND_LIST_USERS, SLACK_OUTBOUND_PROACTIVE,
    SLACK_OUTBOUND_READ_MESSAGES, SLACK_OUTBOUND_READ_REPLIES, SLACK_OUTBOUND_REACTION,
    SLACK_OUTBOUND_SET_STATUS, SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS, SLACK_OUTBOUND_STREAM_APPEND,
    SLACK_OUTBOUND_STREAM_STOP, SLACK_OUTBOUND_UPDATE, SLACK_OUTBOUND_UPLOAD,
    SLACK_OUTBOUND_VIEW_OPEN, SLACK_OUTBOUND_VIEW_PUBLISH,
};
use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn inbound(channel: &str, user: &str, text: &str, session_type: SessionType) -> SlackInboundMessage {
    SlackInboundMessage {
        channel: channel.to_string(),
        user: user.to_string(),
        text: text.to_string(),
        ts: "1700000000.000".to_string(),
        event_ts: None,
        thread_ts: None,
        parent_user_id: None,
        session_type,
        source: None,
        session_key: None,
        files: vec![],
        attachments: vec![],
        display_name: None,
    }
}

fn outbound(channel: &str, text: &str) -> SlackOutboundMessage {
    SlackOutboundMessage {
        channel: channel.to_string(),
        text: text.to_string(),
        thread_ts: None,
        blocks: None,
        username: None,
        icon_url: None,
        icon_emoji: None,
        media_url: None,
    }
}

// ── Inbound pipeline (bot → NATS) ─────────────────────────────────────────────

#[tokio::test]
async fn inbound_dm_published_to_correct_subject() {
    let mock = MockNatsClient::new();
    let msg = inbound("D123", "U456", "hello", SessionType::Direct);
    publish_inbound(&mock, None, &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND]);
}

#[tokio::test]
async fn inbound_channel_published_to_correct_subject() {
    let mock = MockNatsClient::new();
    let msg = inbound("C123", "U456", "hello @bot", SessionType::Channel);
    publish_inbound(&mock, None, &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND]);
}

#[tokio::test]
async fn inbound_payload_deserializes_correctly() {
    let mock = MockNatsClient::new();
    let msg = inbound("D123", "U999", "round-trip text", SessionType::Direct);
    publish_inbound(&mock, None, &msg).await.unwrap();

    // Verify the published payload deserializes back to the same message.
    let payloads = mock.published_payloads();
    assert_eq!(payloads.len(), 1);
    let decoded: SlackInboundMessage = serde_json::from_slice(&payloads[0]).unwrap();
    assert_eq!(decoded.channel, "D123");
    assert_eq!(decoded.user, "U999");
    assert_eq!(decoded.text, "round-trip text");
    assert_eq!(decoded.session_type, SessionType::Direct);
}

// ── Outbound pipeline (agent → NATS) ──────────────────────────────────────────

#[tokio::test]
async fn outbound_published_to_correct_subject() {
    let mock = MockNatsClient::new();
    let msg = outbound("C123", "Here is your answer");
    publish_outbound(&mock, None, &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND]);
}

#[tokio::test]
async fn outbound_payload_deserializes_correctly() {
    let mock = MockNatsClient::new();
    let msg = outbound("C777", "Response text");
    publish_outbound(&mock, None, &msg).await.unwrap();

    let payloads = mock.published_payloads();
    let decoded: SlackOutboundMessage = serde_json::from_slice(&payloads[0]).unwrap();
    assert_eq!(decoded.channel, "C777");
    assert_eq!(decoded.text, "Response text");
}

// ── Multi-account namespacing ─────────────────────────────────────────────────

#[tokio::test]
async fn inbound_namespaced_with_account_id() {
    let mock = MockNatsClient::new();
    let msg = inbound("C1", "U1", "hi", SessionType::Channel);
    publish_inbound(&mock, Some("workspace-a"), &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec!["slack.workspace-a.inbound.message"]);
}

#[tokio::test]
async fn outbound_namespaced_with_account_id() {
    let mock = MockNatsClient::new();
    let msg = outbound("C1", "hi");
    publish_outbound(&mock, Some("acme"), &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec!["slack.acme.outbound.message"]);
}

#[tokio::test]
async fn empty_account_id_uses_bare_subject() {
    let mock = MockNatsClient::new();
    let msg = inbound("C1", "U1", "hi", SessionType::Direct);
    publish_inbound(&mock, Some(""), &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND]);
}

// ── Full bot→agent→bot cycle ──────────────────────────────────────────────────

#[tokio::test]
async fn full_cycle_inbound_then_outbound() {
    let mock = MockNatsClient::new();

    // Bot receives Slack message → publishes to NATS.
    let inbound_msg = inbound("C123", "U456", "What is 2+2?", SessionType::Channel);
    publish_inbound(&mock, None, &inbound_msg).await.unwrap();

    // Agent processes it → publishes response to NATS.
    let outbound_msg = outbound("C123", "The answer is 4.");
    publish_outbound(&mock, None, &outbound_msg).await.unwrap();

    let subjects = mock.published_messages();
    assert_eq!(subjects.len(), 2);
    assert_eq!(subjects[0], SLACK_INBOUND);
    assert_eq!(subjects[1], SLACK_OUTBOUND);
}

#[tokio::test]
async fn full_cycle_with_account_id() {
    let mock = MockNatsClient::new();
    let account = Some("team-x");

    let inbound_msg = inbound("D1", "U1", "hello", SessionType::Direct);
    publish_inbound(&mock, account, &inbound_msg).await.unwrap();

    let outbound_msg = outbound("D1", "hi there");
    publish_outbound(&mock, account, &outbound_msg).await.unwrap();

    let subjects = mock.published_messages();
    assert_eq!(subjects[0], "slack.team-x.inbound.message");
    assert_eq!(subjects[1], "slack.team-x.outbound.message");
}

// ── Streaming pipeline ────────────────────────────────────────────────────────

#[tokio::test]
async fn streaming_append_and_stop_subjects() {
    let mock = MockNatsClient::new();

    let append = SlackStreamAppendMessage {
        channel: "C1".to_string(),
        ts: "1700000001.000".to_string(),
        text: "partial answer…".to_string(),
    };
    publish_stream_append(&mock, None, &append).await.unwrap();

    let stop = SlackStreamStopMessage {
        channel: "C1".to_string(),
        ts: "1700000001.000".to_string(),
        final_text: "final answer".to_string(),
        blocks: None,
    };
    publish_stream_stop(&mock, None, &stop).await.unwrap();

    let subjects = mock.published_messages();
    assert_eq!(subjects[0], SLACK_OUTBOUND_STREAM_APPEND);
    assert_eq!(subjects[1], SLACK_OUTBOUND_STREAM_STOP);
}

#[tokio::test]
async fn streaming_with_account_id() {
    let mock = MockNatsClient::new();
    let append = SlackStreamAppendMessage {
        channel: "C1".to_string(),
        ts: "1.0".to_string(),
        text: "chunk".to_string(),
    };
    publish_stream_append(&mock, Some("ws1"), &append).await.unwrap();
    assert_eq!(mock.published_messages(), vec!["slack.ws1.outbound.stream.append"]);
}

// ── Event types routing ───────────────────────────────────────────────────────

#[tokio::test]
async fn reaction_event_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackReactionEvent {
        reaction: "thumbsup".to_string(),
        user: "U1".to_string(),
        channel: Some("C1".to_string()),
        item_ts: Some("1.0".to_string()),
        item_user: None,
        event_ts: "1700000000.000".to_string(),
        added: true,
    };
    publish_reaction(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_REACTION]);
}

#[tokio::test]
async fn slash_command_event_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackSlashCommandEvent {
        command: "/help".to_string(),
        text: None,
        user_id: "U1".to_string(),
        channel_id: "C1".to_string(),
        team_id: None,
        response_url: "https://hooks.slack.com/commands/xxx".to_string(),
        trigger_id: None,
    };
    publish_slash_command(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_SLASH_COMMAND]);
}

#[tokio::test]
async fn member_event_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackMemberEvent {
        user: "U1".to_string(),
        channel: "C1".to_string(),
        channel_type: None,
        team: None,
        inviter: None,
        joined: true,
    };
    publish_member(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_MEMBER]);
}

#[tokio::test]
async fn channel_event_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackChannelEvent {
        kind: ChannelEventKind::Created,
        channel_id: "C1".to_string(),
        channel_name: Some("general".to_string()),
        user: None,
    };
    publish_channel(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_CHANNEL]);
}

#[tokio::test]
async fn block_action_event_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackBlockActionEvent {
        action_id: "button_click".to_string(),
        block_id: None,
        user_id: "U1".to_string(),
        channel_id: Some("C1".to_string()),
        message_ts: None,
        value: None,
        trigger_id: None,
    };
    publish_block_action(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_BLOCK_ACTION]);
}

// ── Outbound actions ──────────────────────────────────────────────────────────

#[tokio::test]
async fn reaction_action_subject() {
    let mock = MockNatsClient::new();
    let action = SlackReactionAction {
        channel: "C1".to_string(),
        ts: "1.0".to_string(),
        reaction: "white_check_mark".to_string(),
        add: true,
    };
    publish_reaction_action(&mock, None, &action).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_REACTION]);
}

#[tokio::test]
async fn delete_message_subject() {
    let mock = MockNatsClient::new();
    let msg = SlackDeleteMessage {
        channel: "C1".to_string(),
        ts: "1.0".to_string(),
    };
    publish_delete_message(&mock, None, &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_DELETE]);
}

#[tokio::test]
async fn set_status_subject() {
    let mock = MockNatsClient::new();
    let req = SlackSetStatusRequest {
        channel_id: "C1".to_string(),
        thread_ts: "1.0".to_string(),
        status: Some("Thinking…".to_string()),
    };
    publish_set_status(&mock, None, &req).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_SET_STATUS]);
}

#[tokio::test]
async fn ephemeral_message_subject() {
    let mock = MockNatsClient::new();
    let msg = SlackEphemeralMessage {
        channel: "C1".to_string(),
        user: "U1".to_string(),
        text: "Only you can see this".to_string(),
        thread_ts: None,
        blocks: None,
    };
    publish_ephemeral_message(&mock, None, &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_EPHEMERAL]);
}

#[tokio::test]
async fn view_open_subject() {
    let mock = MockNatsClient::new();
    let req = SlackViewOpenRequest {
        trigger_id: "t1".to_string(),
        view: serde_json::json!({"type": "modal"}),
    };
    publish_view_open(&mock, None, &req).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_VIEW_OPEN]);
}

#[tokio::test]
async fn view_publish_subject() {
    let mock = MockNatsClient::new();
    let req = SlackViewPublishRequest {
        user_id: "U1".to_string(),
        view: serde_json::json!({"type": "home", "blocks": []}),
    };
    publish_view_publish(&mock, None, &req).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_VIEW_PUBLISH]);
}

// ── Request/reply (AdvancedMockNatsClient) ────────────────────────────────────

#[tokio::test]
async fn request_read_messages_roundtrip() {
    let mock = AdvancedMockNatsClient::new();

    let response = SlackReadMessagesResponse {
        ok: true,
        messages: vec![SlackReadMessage {
            ts: "1700000000.001".to_string(),
            user: Some("U123".to_string()),
            text: Some("Hello from history".to_string()),
            bot_id: None,
        }],
        error: None,
    };
    mock.set_response(
        SLACK_OUTBOUND_READ_MESSAGES,
        serde_json::to_vec(&response).unwrap().into(),
    );

    let req = SlackReadMessagesRequest {
        channel: "C123".to_string(),
        limit: Some(10),
        oldest: None,
        latest: None,
    };
    let result = request_read_messages(&mock, None, &req).await.unwrap();

    assert!(result.ok);
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].user.as_deref(), Some("U123"));
    assert_eq!(result.messages[0].text.as_deref(), Some("Hello from history"));
}

#[tokio::test]
async fn request_read_messages_with_account_id() {
    let mock = AdvancedMockNatsClient::new();

    let response = SlackReadMessagesResponse { ok: true, messages: vec![], error: None };
    mock.set_response(
        "slack.ws2.outbound.read_messages",
        serde_json::to_vec(&response).unwrap().into(),
    );

    let req = SlackReadMessagesRequest {
        channel: "C1".to_string(),
        limit: None,
        oldest: None,
        latest: None,
    };
    let result = request_read_messages(&mock, Some("ws2"), &req).await.unwrap();
    assert!(result.ok);
}

#[tokio::test]
async fn request_read_replies_roundtrip() {
    let mock = AdvancedMockNatsClient::new();

    let response = SlackReadRepliesResponse {
        ok: true,
        messages: vec![
            SlackReadMessage {
                ts: "1700000000.001".to_string(),
                user: Some("U1".to_string()),
                text: Some("Parent".to_string()),
                bot_id: None,
            },
            SlackReadMessage {
                ts: "1700000000.002".to_string(),
                user: Some("U2".to_string()),
                text: Some("Reply".to_string()),
                bot_id: None,
            },
        ],
        error: None,
    };
    mock.set_response(
        SLACK_OUTBOUND_READ_REPLIES,
        serde_json::to_vec(&response).unwrap().into(),
    );

    let req = SlackReadRepliesRequest {
        channel: "C1".to_string(),
        ts: "1700000000.001".to_string(),
        limit: Some(50),
        oldest: None,
        latest: None,
    };
    let result = request_read_replies(&mock, None, &req).await.unwrap();

    assert!(result.ok);
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[1].text.as_deref(), Some("Reply"));
}

#[tokio::test]
async fn request_list_users_roundtrip() {
    let mock = AdvancedMockNatsClient::new();

    let response = SlackListUsersResponse {
        ok: true,
        members: vec![SlackListUsersUser {
            id: "U001".to_string(),
            name: "alice".to_string(),
            real_name: Some("Alice Smith".to_string()),
            display_name: Some("alice".to_string()),
            is_bot: false,
            deleted: false,
        }],
        next_cursor: None,
        error: None,
    };
    mock.set_response(
        SLACK_OUTBOUND_LIST_USERS,
        serde_json::to_vec(&response).unwrap().into(),
    );

    let req = SlackListUsersRequest { limit: Some(100), cursor: None };
    let result = request_list_users(&mock, None, &req).await.unwrap();

    assert!(result.ok);
    assert_eq!(result.members.len(), 1);
    assert_eq!(result.members[0].id, "U001");
    assert_eq!(result.members[0].name, "alice");
}

#[tokio::test]
async fn request_list_conversations_roundtrip() {
    let mock = AdvancedMockNatsClient::new();

    let response = SlackListConversationsResponse {
        ok: true,
        channels: vec![SlackListConversationsChannel {
            id: "C999".to_string(),
            name: Some("general".to_string()),
            is_channel: true,
            is_private: false,
            is_archived: false,
            num_members: Some(10),
        }],
        next_cursor: None,
        error: None,
    };
    mock.set_response(
        SLACK_OUTBOUND_LIST_CONVERSATIONS,
        serde_json::to_vec(&response).unwrap().into(),
    );

    let req = SlackListConversationsRequest {
        limit: Some(50),
        cursor: None,
        exclude_archived: Some(true),
        types: None,
    };
    let result = request_list_conversations(&mock, None, &req).await.unwrap();

    assert!(result.ok);
    assert_eq!(result.channels.len(), 1);
    assert_eq!(result.channels[0].id, "C999");
}

#[tokio::test]
async fn request_fails_when_no_response_configured() {
    let mock = AdvancedMockNatsClient::new();
    let req = SlackReadMessagesRequest {
        channel: "C1".to_string(),
        limit: None,
        oldest: None,
        latest: None,
    };
    let result = request_read_messages(&mock, None, &req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn request_fails_when_forced() {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_request();

    let req = SlackReadRepliesRequest {
        channel: "C1".to_string(),
        ts: "1.0".to_string(),
        limit: None,
        oldest: None,
        latest: None,
    };
    let result = request_read_replies(&mock, None, &req).await;
    assert!(result.is_err());
}

// ── Payload integrity across multiple events ──────────────────────────────────

#[tokio::test]
async fn multiple_events_tracked_independently() {
    let mock = MockNatsClient::new();

    let dm = inbound("D1", "U1", "msg1", SessionType::Direct);
    let ch = inbound("C1", "U2", "msg2", SessionType::Channel);
    let reply = outbound("D1", "reply1");

    publish_inbound(&mock, None, &dm).await.unwrap();
    publish_inbound(&mock, None, &ch).await.unwrap();
    publish_outbound(&mock, None, &reply).await.unwrap();

    let subjects = mock.published_messages();
    assert_eq!(subjects.len(), 3);
    assert_eq!(subjects[0], SLACK_INBOUND);
    assert_eq!(subjects[1], SLACK_INBOUND);
    assert_eq!(subjects[2], SLACK_OUTBOUND);

    let payloads = mock.published_payloads();
    let decoded_dm: SlackInboundMessage = serde_json::from_slice(&payloads[0]).unwrap();
    let decoded_ch: SlackInboundMessage = serde_json::from_slice(&payloads[1]).unwrap();
    let decoded_reply: SlackOutboundMessage = serde_json::from_slice(&payloads[2]).unwrap();

    assert_eq!(decoded_dm.user, "U1");
    assert_eq!(decoded_ch.user, "U2");
    assert_eq!(decoded_reply.text, "reply1");
}

// ── Inbound event types (remaining) ──────────────────────────────────────────

#[tokio::test]
async fn message_changed_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackMessageChangedEvent {
        channel: "C1".to_string(),
        ts: "1.0".to_string(),
        event_ts: None,
        previous_text: Some("old".to_string()),
        new_text: Some("new".to_string()),
        thread_ts: None,
        user: Some("U1".to_string()),
    };
    publish_message_changed(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_MESSAGE_CHANGED]);
}

#[tokio::test]
async fn message_deleted_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackMessageDeletedEvent {
        channel: "C1".to_string(),
        deleted_ts: "1.0".to_string(),
        event_ts: None,
        thread_ts: None,
    };
    publish_message_deleted(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_MESSAGE_DELETED]);
}

#[tokio::test]
async fn thread_broadcast_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackThreadBroadcastEvent {
        channel: "C1".to_string(),
        user: "U1".to_string(),
        text: "broadcast".to_string(),
        ts: "1.0".to_string(),
        thread_ts: "0.9".to_string(),
        event_ts: None,
    };
    publish_thread_broadcast(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_THREAD_BROADCAST]);
}

#[tokio::test]
async fn app_home_opened_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackAppHomeOpenedEvent {
        user: "U1".to_string(),
        tab: "home".to_string(),
        view_id: None,
    };
    publish_app_home(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_APP_HOME]);
}

#[tokio::test]
async fn view_submission_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackViewSubmissionEvent {
        user_id: "U1".to_string(),
        trigger_id: "t1".to_string(),
        view_id: "V1".to_string(),
        callback_id: Some("settings_modal".to_string()),
        values: serde_json::json!({}),
    };
    publish_view_submission(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_VIEW_SUBMISSION]);
}

#[tokio::test]
async fn view_closed_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackViewClosedEvent {
        user_id: "U1".to_string(),
        trigger_id: "t1".to_string(),
        view_id: "V1".to_string(),
        callback_id: None,
        values: serde_json::json!({}),
    };
    publish_view_closed(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_VIEW_CLOSED]);
}

#[tokio::test]
async fn pin_event_subject() {
    let mock = MockNatsClient::new();
    let ev = SlackPinEvent {
        kind: PinEventKind::Added,
        channel: "C1".to_string(),
        user: "U1".to_string(),
        item_ts: Some("1.0".to_string()),
        item_type: Some("message".to_string()),
        event_ts: "1700000000.000".to_string(),
    };
    publish_pin(&mock, None, &ev).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_INBOUND_PIN]);
}

// ── Outbound actions (remaining) ──────────────────────────────────────────────

#[tokio::test]
async fn update_message_subject() {
    let mock = MockNatsClient::new();
    let msg = SlackUpdateMessage {
        channel: "C1".to_string(),
        ts: "1.0".to_string(),
        text: "updated text".to_string(),
        blocks: None,
    };
    publish_update_message(&mock, None, &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_UPDATE]);
}

#[tokio::test]
async fn upload_request_subject() {
    let mock = MockNatsClient::new();
    let req = SlackUploadRequest {
        channel: "C1".to_string(),
        thread_ts: None,
        filename: "report.md".to_string(),
        content: "# Report\nContent here".to_string(),
        title: Some("Monthly Report".to_string()),
    };
    publish_upload_request(&mock, None, &req).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_UPLOAD]);
}

#[tokio::test]
async fn set_suggested_prompts_subject() {
    let mock = MockNatsClient::new();
    let req = SlackSetSuggestedPromptsRequest {
        channel_id: "C1".to_string(),
        thread_ts: "1.0".to_string(),
        title: Some("Try asking:".to_string()),
        prompts: vec![
            SlackSuggestedPrompt {
                title: "Summarize".to_string(),
                message: "Summarize this thread".to_string(),
            },
        ],
    };
    publish_set_suggested_prompts(&mock, None, &req).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS]);
}

#[tokio::test]
async fn proactive_message_subject() {
    let mock = MockNatsClient::new();
    let msg = SlackProactiveMessage {
        channel: Some("C1".to_string()),
        user_id: None,
        text: "Heads up!".to_string(),
        thread_ts: None,
        blocks: None,
        username: None,
        icon_url: None,
        icon_emoji: None,
    };
    publish_proactive_message(&mock, None, &msg).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_PROACTIVE]);
}

#[tokio::test]
async fn delete_file_subject() {
    let mock = MockNatsClient::new();
    let req = SlackDeleteFile { file_id: "F1234".to_string() };
    publish_delete_file(&mock, None, &req).await.unwrap();
    assert_eq!(mock.published_messages(), vec![SLACK_OUTBOUND_DELETE_FILE]);
}

// ── All subjects covered: namespacing check ───────────────────────────────────

#[tokio::test]
async fn all_inbound_events_namespaced_correctly() {
    // Verify that account_id namespacing works for every inbound event type.
    let mock = MockNatsClient::new();
    let account = Some("org");

    publish_inbound(&mock, account, &inbound("C1", "U1", "hi", SessionType::Direct)).await.unwrap();
    publish_reaction(&mock, account, &SlackReactionEvent {
        reaction: "eyes".to_string(), user: "U1".to_string(), channel: None,
        item_ts: None, item_user: None, event_ts: "1.0".to_string(), added: true,
    }).await.unwrap();
    publish_message_changed(&mock, account, &SlackMessageChangedEvent {
        channel: "C1".to_string(), ts: "1.0".to_string(), event_ts: None,
        previous_text: None, new_text: Some("x".to_string()), thread_ts: None, user: None,
    }).await.unwrap();
    publish_message_deleted(&mock, account, &SlackMessageDeletedEvent {
        channel: "C1".to_string(), deleted_ts: "1.0".to_string(), event_ts: None, thread_ts: None,
    }).await.unwrap();
    publish_slash_command(&mock, account, &SlackSlashCommandEvent {
        command: "/cmd".to_string(), text: None, user_id: "U1".to_string(),
        channel_id: "C1".to_string(), team_id: None,
        response_url: "https://hooks.slack.com/x".to_string(), trigger_id: None,
    }).await.unwrap();

    let subjects = mock.published_messages();
    assert!(subjects.iter().all(|s| s.starts_with("slack.org.")),
        "All subjects should be namespaced under slack.org.*: {:?}", subjects);
}

#[tokio::test]
async fn all_outbound_events_namespaced_correctly() {
    let mock = MockNatsClient::new();
    let account = Some("team");

    publish_outbound(&mock, account, &outbound("C1", "reply")).await.unwrap();
    publish_stream_append(&mock, account, &SlackStreamAppendMessage {
        channel: "C1".to_string(), ts: "1.0".to_string(), text: "chunk".to_string(),
    }).await.unwrap();
    publish_stream_stop(&mock, account, &SlackStreamStopMessage {
        channel: "C1".to_string(), ts: "1.0".to_string(),
        final_text: "done".to_string(), blocks: None,
    }).await.unwrap();
    publish_reaction_action(&mock, account, &SlackReactionAction {
        channel: "C1".to_string(), ts: "1.0".to_string(), reaction: "ok".to_string(), add: true,
    }).await.unwrap();
    publish_delete_message(&mock, account, &SlackDeleteMessage {
        channel: "C1".to_string(), ts: "1.0".to_string(),
    }).await.unwrap();

    let subjects = mock.published_messages();
    assert!(subjects.iter().all(|s| s.starts_with("slack.team.")),
        "All outbound subjects should be namespaced under slack.team.*: {:?}", subjects);
}

// ── Error handling ────────────────────────────────────────────────────────────

#[tokio::test]
async fn request_read_messages_malformed_response_returns_error() {
    let mock = AdvancedMockNatsClient::new();
    // Configure a response that is not valid JSON for SlackReadMessagesResponse.
    mock.set_response(SLACK_OUTBOUND_READ_MESSAGES, b"not valid json at all".as_slice().into());

    let req = SlackReadMessagesRequest { channel: "C1".to_string(), limit: None, oldest: None, latest: None };
    let result = request_read_messages(&mock, None, &req).await;
    assert!(result.is_err(), "Malformed JSON response should return an error");
}

#[tokio::test]
async fn request_read_replies_malformed_response_returns_error() {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response(SLACK_OUTBOUND_READ_REPLIES, b"{\"invalid\": true}".as_slice().into());

    let req = SlackReadRepliesRequest {
        channel: "C1".to_string(), ts: "1.0".to_string(),
        limit: None, oldest: None, latest: None,
    };
    // Missing required fields → deserialization should fail.
    let result = request_read_replies(&mock, None, &req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn request_list_users_no_response_returns_error() {
    let mock = AdvancedMockNatsClient::new();
    let req = SlackListUsersRequest { limit: None, cursor: None };
    let result = request_list_users(&mock, None, &req).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn request_list_conversations_no_response_returns_error() {
    let mock = AdvancedMockNatsClient::new();
    let req = SlackListConversationsRequest { limit: None, cursor: None, exclude_archived: None, types: None };
    let result = request_list_conversations(&mock, None, &req).await;
    assert!(result.is_err());
}

// ── Payload integrity for new event types ────────────────────────────────────

#[tokio::test]
async fn message_changed_payload_roundtrip() {
    let mock = MockNatsClient::new();
    let ev = SlackMessageChangedEvent {
        channel: "C99".to_string(),
        ts: "9.0".to_string(),
        event_ts: Some("9.1".to_string()),
        previous_text: Some("before".to_string()),
        new_text: Some("after".to_string()),
        thread_ts: Some("8.0".to_string()),
        user: Some("U99".to_string()),
    };
    publish_message_changed(&mock, None, &ev).await.unwrap();

    let payloads = mock.published_payloads();
    let decoded: SlackMessageChangedEvent = serde_json::from_slice(&payloads[0]).unwrap();
    assert_eq!(decoded.channel, "C99");
    assert_eq!(decoded.new_text.as_deref(), Some("after"));
    assert_eq!(decoded.user.as_deref(), Some("U99"));
}

#[tokio::test]
async fn view_submission_payload_roundtrip() {
    let mock = MockNatsClient::new();
    let ev = SlackViewSubmissionEvent {
        user_id: "U42".to_string(),
        trigger_id: "trigger42".to_string(),
        view_id: "V42".to_string(),
        callback_id: Some("my_modal".to_string()),
        values: serde_json::json!({"block1": {"action1": {"value": "hello"}}}),
    };
    publish_view_submission(&mock, None, &ev).await.unwrap();

    let payloads = mock.published_payloads();
    let decoded: SlackViewSubmissionEvent = serde_json::from_slice(&payloads[0]).unwrap();
    assert_eq!(decoded.user_id, "U42");
    assert_eq!(decoded.callback_id.as_deref(), Some("my_modal"));
}

#[tokio::test]
async fn pin_event_payload_roundtrip() {
    let mock = MockNatsClient::new();
    let ev = SlackPinEvent {
        kind: PinEventKind::Removed,
        channel: "C7".to_string(),
        user: "U7".to_string(),
        item_ts: Some("7.0".to_string()),
        item_type: Some("message".to_string()),
        event_ts: "7.1".to_string(),
    };
    publish_pin(&mock, None, &ev).await.unwrap();

    let payloads = mock.published_payloads();
    let decoded: SlackPinEvent = serde_json::from_slice(&payloads[0]).unwrap();
    assert_eq!(decoded.channel, "C7");
    assert_eq!(decoded.item_ts.as_deref(), Some("7.0"));
}
