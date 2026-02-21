use serde::Serialize;
use slack_types::events::{
    SlackAppHomeOpenedEvent, SlackBlockActionEvent, SlackChannelEvent, SlackDeleteFile,
    SlackDeleteMessage, SlackEphemeralMessage, SlackGetEmojiRequest, SlackGetEmojiResponse,
    SlackGetUserRequest, SlackGetUserResponse, SlackInboundMessage, SlackLinkSharedEvent,
    SlackMemberEvent, SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackOutboundMessage,
    SlackPinEvent, SlackProactiveMessage, SlackReactionAction, SlackReactionEvent,
    SlackReadMessagesRequest, SlackReadMessagesResponse, SlackReadRepliesRequest,
    SlackReadRepliesResponse, SlackResponseUrlMessage, SlackSetStatusRequest,
    SlackSetSuggestedPromptsRequest, SlackSlashCommandEvent, SlackStreamAppendMessage,
    SlackStreamStopMessage, SlackThreadBroadcastEvent, SlackUnfurlRequest, SlackUpdateMessage,
    SlackUploadRequest, SlackViewClosedEvent, SlackViewOpenRequest, SlackViewPublishRequest,
    SlackViewSubmissionEvent,
};
use slack_types::subjects::{
    for_account, SLACK_INBOUND, SLACK_INBOUND_APP_HOME, SLACK_INBOUND_BLOCK_ACTION,
    SLACK_INBOUND_CHANNEL, SLACK_INBOUND_LINK_SHARED, SLACK_INBOUND_MEMBER,
    SLACK_INBOUND_MESSAGE_CHANGED, SLACK_INBOUND_MESSAGE_DELETED, SLACK_INBOUND_PIN,
    SLACK_INBOUND_REACTION, SLACK_INBOUND_SLASH_COMMAND, SLACK_INBOUND_THREAD_BROADCAST,
    SLACK_INBOUND_VIEW_CLOSED, SLACK_INBOUND_VIEW_SUBMISSION, SLACK_OUTBOUND,
    SLACK_OUTBOUND_DELETE, SLACK_OUTBOUND_DELETE_FILE, SLACK_OUTBOUND_EPHEMERAL,
    SLACK_OUTBOUND_GET_EMOJI, SLACK_OUTBOUND_GET_USER, SLACK_OUTBOUND_LIST_CONVERSATIONS,
    SLACK_OUTBOUND_LIST_USERS, SLACK_OUTBOUND_PROACTIVE, SLACK_OUTBOUND_REACTION,
    SLACK_OUTBOUND_READ_MESSAGES, SLACK_OUTBOUND_READ_REPLIES, SLACK_OUTBOUND_RESPONSE_URL,
    SLACK_OUTBOUND_SET_STATUS, SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS,
    SLACK_OUTBOUND_STREAM_APPEND, SLACK_OUTBOUND_STREAM_STOP, SLACK_OUTBOUND_UNFURL,
    SLACK_OUTBOUND_UPDATE, SLACK_OUTBOUND_UPLOAD, SLACK_OUTBOUND_VIEW_OPEN,
    SLACK_OUTBOUND_VIEW_PUBLISH,
};
use trogon_nats::{PublishClient, RequestClient};

async fn js_publish<C, T>(
    client: &C,
    subject: &str,
    msg: &T,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
    T: Serialize,
{
    let payload = serde_json::to_vec(msg)?;
    client
        .publish_with_headers(subject.to_string(), async_nats::HeaderMap::new(), payload.into())
        .await
        .map_err(|e| -> async_nats::Error { Box::new(e) })?;
    Ok(())
}

pub async fn publish_inbound<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackInboundMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND, account_id), msg).await
}

pub async fn publish_reaction<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackReactionEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_REACTION, account_id), ev).await
}

pub async fn publish_message_changed<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackMessageChangedEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_MESSAGE_CHANGED, account_id), ev).await
}

pub async fn publish_message_deleted<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackMessageDeletedEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_MESSAGE_DELETED, account_id), ev).await
}

pub async fn publish_thread_broadcast<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackThreadBroadcastEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_THREAD_BROADCAST, account_id), ev).await
}

pub async fn publish_member<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackMemberEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_MEMBER, account_id), ev).await
}

pub async fn publish_channel<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackChannelEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_CHANNEL, account_id), ev).await
}

pub async fn publish_slash_command<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackSlashCommandEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_SLASH_COMMAND, account_id), ev).await
}

// ── Outbound publishers (agent → NATS → slack-bot → Slack) ──────────────────

/// Publish a one-shot outbound message (`chat.postMessage`).
pub async fn publish_outbound<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackOutboundMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND, account_id), msg).await
}

/// Publish an in-flight streaming update (`chat.update` — append step).
pub async fn publish_stream_append<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackStreamAppendMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_STREAM_APPEND, account_id), msg).await
}

/// Publish the final streaming update (`chat.update` — stop step).
pub async fn publish_stream_stop<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackStreamStopMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_STREAM_STOP, account_id), msg).await
}

pub async fn publish_block_action<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackBlockActionEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_BLOCK_ACTION, account_id), ev).await
}

/// Publish a reaction action (add or remove an emoji on a message).
pub async fn publish_reaction_action<C>(
    js: &C,
    account_id: Option<&str>,
    action: &SlackReactionAction,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_REACTION, account_id), action).await
}

pub async fn publish_app_home<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackAppHomeOpenedEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_APP_HOME, account_id), ev).await
}

pub async fn publish_view_submission<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackViewSubmissionEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_VIEW_SUBMISSION, account_id), ev).await
}

/// Publish a request to open a modal (`views.open`).
pub async fn publish_view_open<C>(
    js: &C,
    account_id: Option<&str>,
    req: &SlackViewOpenRequest,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_VIEW_OPEN, account_id), req).await
}

/// Publish a request to update the App Home view (`views.publish`).
pub async fn publish_view_publish<C>(
    js: &C,
    account_id: Option<&str>,
    req: &SlackViewPublishRequest,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_VIEW_PUBLISH, account_id), req).await
}

pub async fn publish_view_closed<C>(
    js: &C,
    account_id: Option<&str>,
    event: &SlackViewClosedEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_VIEW_CLOSED, account_id), event).await
}

pub async fn publish_pin<C>(
    js: &C,
    account_id: Option<&str>,
    event: &SlackPinEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_PIN, account_id), event).await
}

pub async fn publish_set_status<C>(
    js: &C,
    account_id: Option<&str>,
    req: &SlackSetStatusRequest,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_SET_STATUS, account_id), req).await
}

pub async fn publish_delete_message<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackDeleteMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_DELETE, account_id), msg).await
}

pub async fn publish_update_message<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackUpdateMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_UPDATE, account_id), msg).await
}

/// Fetch channel history from the bot via Core NATS request/reply.
pub async fn request_read_messages<C>(
    client: &C,
    account_id: Option<&str>,
    req: &SlackReadMessagesRequest,
) -> Result<SlackReadMessagesResponse, async_nats::Error>
where
    C: RequestClient,
    C::RequestError: 'static,
{
    let subject = for_account(SLACK_OUTBOUND_READ_MESSAGES, account_id);
    let payload = serde_json::to_vec(req)?;
    let response = client
        .request_with_headers(subject, async_nats::HeaderMap::new(), payload.into())
        .await
        .map_err(|e| -> async_nats::Error { Box::new(e) })?;
    let result: SlackReadMessagesResponse = serde_json::from_slice(&response.payload)?;
    Ok(result)
}

/// Publish a request to upload content as a Slack file.
pub async fn publish_upload_request<C>(
    js: &C,
    account_id: Option<&str>,
    req: &SlackUploadRequest,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_UPLOAD, account_id), req).await
}

/// Fetch thread replies from the bot via Core NATS request/reply.
pub async fn request_read_replies<C>(
    client: &C,
    account_id: Option<&str>,
    req: &SlackReadRepliesRequest,
) -> Result<SlackReadRepliesResponse, async_nats::Error>
where
    C: RequestClient,
    C::RequestError: 'static,
{
    let subject = for_account(SLACK_OUTBOUND_READ_REPLIES, account_id);
    let payload = serde_json::to_vec(req)?;
    let response = client
        .request_with_headers(subject, async_nats::HeaderMap::new(), payload.into())
        .await
        .map_err(|e| -> async_nats::Error { Box::new(e) })?;
    let result: SlackReadRepliesResponse = serde_json::from_slice(&response.payload)?;
    Ok(result)
}

/// Publish a request to set suggested prompts on an assistant thread.
pub async fn publish_set_suggested_prompts<C>(
    js: &C,
    account_id: Option<&str>,
    req: &SlackSetSuggestedPromptsRequest,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS, account_id), req).await
}

/// Publish a proactive message (no inbound trigger required).
pub async fn publish_proactive_message<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackProactiveMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_PROACTIVE, account_id), msg).await
}

/// Publish an ephemeral message (visible only to a specific user).
pub async fn publish_ephemeral_message<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackEphemeralMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_EPHEMERAL, account_id), msg).await
}

/// Publish a request to delete an uploaded file.
pub async fn publish_delete_file<C>(
    js: &C,
    account_id: Option<&str>,
    req: &SlackDeleteFile,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_DELETE_FILE, account_id), req).await
}

/// Publish a `link_shared` event (one or more URLs shared in a message).
pub async fn publish_link_shared<C>(
    js: &C,
    account_id: Option<&str>,
    ev: &SlackLinkSharedEvent,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_INBOUND_LINK_SHARED, account_id), ev).await
}

/// Publish an unfurl request — instructs the bot to call `chat.unfurl`.
pub async fn publish_unfurl<C>(
    js: &C,
    account_id: Option<&str>,
    req: &SlackUnfurlRequest,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_UNFURL, account_id), req).await
}

/// Fetch a single user's profile from the bot via Core NATS request/reply.
pub async fn request_get_user<C>(
    client: &C,
    account_id: Option<&str>,
    req: &SlackGetUserRequest,
) -> Result<SlackGetUserResponse, String>
where
    C: RequestClient,
    C::RequestError: 'static,
{
    let subject = for_account(SLACK_OUTBOUND_GET_USER, account_id);
    let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let response = client
        .request_with_headers(subject, async_nats::HeaderMap::new(), payload.into())
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&response.payload).map_err(|e| e.to_string())
}

/// Fetch the workspace custom emoji map from the bot via Core NATS request/reply.
pub async fn request_get_emoji<C>(
    client: &C,
    account_id: Option<&str>,
    req: &SlackGetEmojiRequest,
) -> Result<SlackGetEmojiResponse, String>
where
    C: RequestClient,
    C::RequestError: 'static,
{
    let subject = for_account(SLACK_OUTBOUND_GET_EMOJI, account_id);
    let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let response = client
        .request_with_headers(subject, async_nats::HeaderMap::new(), payload.into())
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&response.payload).map_err(|e| e.to_string())
}

/// List workspace users from the bot via Core NATS request/reply.
pub async fn request_list_users<C>(
    client: &C,
    account_id: Option<&str>,
    req: &slack_types::events::SlackListUsersRequest,
) -> Result<slack_types::events::SlackListUsersResponse, String>
where
    C: RequestClient,
    C::RequestError: 'static,
{
    let subject = for_account(SLACK_OUTBOUND_LIST_USERS, account_id);
    let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let response = client
        .request_with_headers(subject, async_nats::HeaderMap::new(), payload.into())
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&response.payload).map_err(|e| e.to_string())
}

/// Publish a response_url message — instructs the bot to POST to the Slack webhook URL.
pub async fn publish_response_url<C>(
    js: &C,
    account_id: Option<&str>,
    msg: &SlackResponseUrlMessage,
) -> Result<(), async_nats::Error>
where
    C: PublishClient,
    C::PublishError: 'static,
{
    js_publish(js, &for_account(SLACK_OUTBOUND_RESPONSE_URL, account_id), msg).await
}

/// List channels/conversations from the bot via Core NATS request/reply.
pub async fn request_list_conversations<C>(
    client: &C,
    account_id: Option<&str>,
    req: &slack_types::events::SlackListConversationsRequest,
) -> Result<slack_types::events::SlackListConversationsResponse, String>
where
    C: RequestClient,
    C::RequestError: 'static,
{
    let subject = for_account(SLACK_OUTBOUND_LIST_CONVERSATIONS, account_id);
    let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let response = client
        .request_with_headers(subject, async_nats::HeaderMap::new(), payload.into())
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&response.payload).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_types::events::{SessionType, SlackInboundMessage};
    use trogon_nats::MockNatsClient;

    fn make_inbound(channel: &str, user: &str, text: &str) -> SlackInboundMessage {
        SlackInboundMessage {
            channel: channel.to_string(),
            user: user.to_string(),
            text: text.to_string(),
            ts: "1234567890.000".to_string(),
            event_ts: None,
            thread_ts: None,
            parent_user_id: None,
            session_type: SessionType::Direct,
            source: None,
            session_key: None,
            files: vec![],
            attachments: vec![],
            display_name: None,
        }
    }

    #[tokio::test]
    async fn publish_inbound_routes_to_correct_subject() {
        let mock = MockNatsClient::new();
        let msg = make_inbound("C123", "U456", "hello");
        publish_inbound(&mock, None, &msg).await.unwrap();
        let msgs = mock.published_messages();
        assert_eq!(msgs, vec!["slack.inbound.message"]);
    }

    #[tokio::test]
    async fn publish_inbound_namespaces_with_account_id() {
        let mock = MockNatsClient::new();
        let msg = make_inbound("C123", "U456", "hello");
        publish_inbound(&mock, Some("ws1"), &msg).await.unwrap();
        let msgs = mock.published_messages();
        assert_eq!(msgs, vec!["slack.ws1.inbound.message"]);
    }

    #[tokio::test]
    async fn publish_outbound_routes_to_correct_subject() {
        use slack_types::events::SlackOutboundMessage;
        let mock = MockNatsClient::new();
        let msg = SlackOutboundMessage {
            channel: "C123".to_string(),
            text: "hi".to_string(),
            thread_ts: None,
            blocks: None,
            username: None,
            icon_url: None,
            icon_emoji: None,
            media_url: None,
        };
        publish_outbound(&mock, None, &msg).await.unwrap();
        assert_eq!(mock.published_messages(), vec!["slack.outbound.message"]);
    }

    #[tokio::test]
    async fn publish_outbound_with_account_id() {
        use slack_types::events::SlackOutboundMessage;
        let mock = MockNatsClient::new();
        let msg = SlackOutboundMessage {
            channel: "C123".to_string(),
            text: "hi".to_string(),
            thread_ts: None,
            blocks: None,
            username: None,
            icon_url: None,
            icon_emoji: None,
            media_url: None,
        };
        publish_outbound(&mock, Some("acme"), &msg).await.unwrap();
        assert_eq!(mock.published_messages(), vec!["slack.acme.outbound.message"]);
    }

    #[tokio::test]
    async fn publish_stream_append_routes_correctly() {
        use slack_types::events::SlackStreamAppendMessage;
        let mock = MockNatsClient::new();
        let msg = SlackStreamAppendMessage {
            channel: "C1".to_string(),
            ts: "1.0".to_string(),
            text: "chunk".to_string(),
        };
        publish_stream_append(&mock, None, &msg).await.unwrap();
        assert_eq!(mock.published_messages(), vec!["slack.outbound.stream.append"]);
    }

    #[tokio::test]
    async fn multiple_publishes_tracked() {
        let mock = MockNatsClient::new();
        let msg = make_inbound("C1", "U1", "a");
        publish_inbound(&mock, None, &msg).await.unwrap();
        publish_inbound(&mock, Some("ws2"), &msg).await.unwrap();
        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], "slack.inbound.message");
        assert_eq!(msgs[1], "slack.ws2.inbound.message");
    }
}
