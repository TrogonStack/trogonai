use async_nats::jetstream::{
    Context,
    consumer::{Consumer, pull},
};
use slack_types::subjects::{
    for_account, SLACK_INBOUND, SLACK_INBOUND_APP_HOME, SLACK_INBOUND_BLOCK_ACTION,
    SLACK_INBOUND_CHANNEL, SLACK_INBOUND_LINK_SHARED, SLACK_INBOUND_MEMBER,
    SLACK_INBOUND_MESSAGE_CHANGED, SLACK_INBOUND_MESSAGE_DELETED, SLACK_INBOUND_PIN,
    SLACK_INBOUND_REACTION, SLACK_INBOUND_SLASH_COMMAND, SLACK_INBOUND_THREAD_BROADCAST,
    SLACK_INBOUND_VIEW_CLOSED, SLACK_INBOUND_VIEW_SUBMISSION, SLACK_OUTBOUND,
    SLACK_OUTBOUND_DELETE, SLACK_OUTBOUND_DELETE_FILE, SLACK_OUTBOUND_EPHEMERAL,
    SLACK_OUTBOUND_PROACTIVE, SLACK_OUTBOUND_REACTION, SLACK_OUTBOUND_RESPONSE_URL,
    SLACK_OUTBOUND_SET_STATUS, SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS,
    SLACK_OUTBOUND_STREAM_APPEND, SLACK_OUTBOUND_STREAM_STOP, SLACK_OUTBOUND_UNFURL,
    SLACK_OUTBOUND_UPDATE, SLACK_OUTBOUND_UPLOAD, SLACK_OUTBOUND_VIEW_OPEN,
    SLACK_OUTBOUND_VIEW_PUBLISH,
};

use crate::setup::STREAM_NAME;

async fn make_consumer(
    js: &Context,
    durable_name: &str,
    filter_subject: &str,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    let stream = js.get_stream(STREAM_NAME).await?;
    let consumer = stream
        .get_or_create_consumer(
            durable_name,
            pull::Config {
                durable_name: Some(durable_name.to_string()),
                filter_subject: filter_subject.to_string(),
                ..Default::default()
            },
        )
        .await?;
    Ok(consumer)
}

/// Returns a durable consumer name that includes the account ID when set.
/// E.g. base="slack-bot-outbound", account_id=Some("ws1") → "ws1-slack-bot-outbound".
fn consumer_name(base: &str, account_id: Option<&str>) -> String {
    match account_id.filter(|id| !id.is_empty()) {
        Some(id) => format!("{}-{}", id, base),
        None => base.to_string(),
    }
}

// ── Outbound consumers (used by slack-bot) ───────────────────────────────────

pub async fn create_outbound_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-outbound", account_id),
        &for_account(SLACK_OUTBOUND, account_id),
    )
    .await
}

pub async fn create_stream_append_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-stream-append", account_id),
        &for_account(SLACK_OUTBOUND_STREAM_APPEND, account_id),
    )
    .await
}

pub async fn create_stream_stop_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-stream-stop", account_id),
        &for_account(SLACK_OUTBOUND_STREAM_STOP, account_id),
    )
    .await
}

pub async fn create_reaction_action_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-reaction-action", account_id),
        &for_account(SLACK_OUTBOUND_REACTION, account_id),
    )
    .await
}

// ── Inbound consumers (used by slack-agent) ──────────────────────────────────

pub async fn create_inbound_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-inbound", account_id),
        &for_account(SLACK_INBOUND, account_id),
    )
    .await
}

pub async fn create_reaction_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-reaction", account_id),
        &for_account(SLACK_INBOUND_REACTION, account_id),
    )
    .await
}

pub async fn create_message_changed_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-message-changed", account_id),
        &for_account(SLACK_INBOUND_MESSAGE_CHANGED, account_id),
    )
    .await
}

pub async fn create_message_deleted_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-message-deleted", account_id),
        &for_account(SLACK_INBOUND_MESSAGE_DELETED, account_id),
    )
    .await
}

pub async fn create_slash_command_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-slash-command", account_id),
        &for_account(SLACK_INBOUND_SLASH_COMMAND, account_id),
    )
    .await
}

pub async fn create_block_action_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-block-action", account_id),
        &for_account(SLACK_INBOUND_BLOCK_ACTION, account_id),
    )
    .await
}

pub async fn create_thread_broadcast_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-thread-broadcast", account_id),
        &for_account(SLACK_INBOUND_THREAD_BROADCAST, account_id),
    )
    .await
}

pub async fn create_member_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-member", account_id),
        &for_account(SLACK_INBOUND_MEMBER, account_id),
    )
    .await
}

pub async fn create_channel_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-channel", account_id),
        &for_account(SLACK_INBOUND_CHANNEL, account_id),
    )
    .await
}

pub async fn create_app_home_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-app-home", account_id),
        &for_account(SLACK_INBOUND_APP_HOME, account_id),
    )
    .await
}

pub async fn create_view_submission_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-view-submission", account_id),
        &for_account(SLACK_INBOUND_VIEW_SUBMISSION, account_id),
    )
    .await
}

// ── Bot consumers for view outbound ──────────────────────────────────────────

pub async fn create_view_open_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-view-open", account_id),
        &for_account(SLACK_OUTBOUND_VIEW_OPEN, account_id),
    )
    .await
}

pub async fn create_view_publish_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-view-publish", account_id),
        &for_account(SLACK_OUTBOUND_VIEW_PUBLISH, account_id),
    )
    .await
}

pub async fn create_view_closed_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-view-closed", account_id),
        &for_account(SLACK_INBOUND_VIEW_CLOSED, account_id),
    )
    .await
}

pub async fn create_pin_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-pin", account_id),
        &for_account(SLACK_INBOUND_PIN, account_id),
    )
    .await
}

pub async fn create_set_status_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-set-status", account_id),
        &for_account(SLACK_OUTBOUND_SET_STATUS, account_id),
    )
    .await
}

pub async fn create_delete_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-delete", account_id),
        &for_account(SLACK_OUTBOUND_DELETE, account_id),
    )
    .await
}

pub async fn create_update_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-update", account_id),
        &for_account(SLACK_OUTBOUND_UPDATE, account_id),
    )
    .await
}

pub async fn create_upload_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-upload", account_id),
        &for_account(SLACK_OUTBOUND_UPLOAD, account_id),
    )
    .await
}

pub async fn create_suggested_prompts_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-suggested-prompts", account_id),
        &for_account(SLACK_OUTBOUND_SET_SUGGESTED_PROMPTS, account_id),
    )
    .await
}

pub async fn create_proactive_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-proactive", account_id),
        &for_account(SLACK_OUTBOUND_PROACTIVE, account_id),
    )
    .await
}

pub async fn create_ephemeral_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-ephemeral", account_id),
        &for_account(SLACK_OUTBOUND_EPHEMERAL, account_id),
    )
    .await
}

pub async fn create_delete_file_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-delete-file", account_id),
        &for_account(SLACK_OUTBOUND_DELETE_FILE, account_id),
    )
    .await
}

/// JetStream consumer for `link_shared` events (inbound — used by the agent).
pub async fn create_link_shared_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-agent-link-shared", account_id),
        &for_account(SLACK_INBOUND_LINK_SHARED, account_id),
    )
    .await
}

/// JetStream consumer for unfurl requests (outbound — used by the bot).
pub async fn create_unfurl_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-unfurl", account_id),
        &for_account(SLACK_OUTBOUND_UNFURL, account_id),
    )
    .await
}

/// JetStream consumer for response_url messages (outbound — used by the bot).
pub async fn create_response_url_consumer(
    js: &Context,
    account_id: Option<&str>,
) -> Result<Consumer<pull::Config>, async_nats::Error> {
    make_consumer(
        js,
        &consumer_name("slack-bot-response-url", account_id),
        &for_account(SLACK_OUTBOUND_RESPONSE_URL, account_id),
    )
    .await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── consumer_name ─────────────────────────────────────────────────────────

    #[test]
    fn consumer_name_no_account_returns_base() {
        assert_eq!(consumer_name("slack-bot-outbound", None), "slack-bot-outbound");
    }

    #[test]
    fn consumer_name_empty_account_returns_base() {
        assert_eq!(consumer_name("slack-bot-outbound", Some("")), "slack-bot-outbound");
    }

    #[test]
    fn consumer_name_with_account_id_prepends_prefix() {
        assert_eq!(
            consumer_name("slack-bot-outbound", Some("ws1")),
            "ws1-slack-bot-outbound"
        );
    }

    #[test]
    fn consumer_name_with_account_id_different_bases() {
        assert_eq!(consumer_name("slack-agent-inbound", Some("acme")), "acme-slack-agent-inbound");
        assert_eq!(consumer_name("slack-bot-stream-append", Some("dev")), "dev-slack-bot-stream-append");
        assert_eq!(consumer_name("slack-agent-pin", Some("prod")), "prod-slack-agent-pin");
    }

    #[test]
    fn consumer_name_whitespace_only_account_is_treated_as_non_empty() {
        // filter(|id| !id.is_empty()) only checks for empty string, not whitespace.
        // Whitespace-only ids would be prepended as-is (documenting behaviour).
        let result = consumer_name("base", Some("  "));
        assert_eq!(result, "  -base");
    }

    #[test]
    fn consumer_name_all_consumers_have_unique_base_names() {
        // Ensure none of the consumer base names used in this module clash with
        // each other — a naming collision would cause two consumers to share the
        // same durable name and steal each other's messages.
        let bases = [
            "slack-bot-outbound",
            "slack-bot-stream-append",
            "slack-bot-stream-stop",
            "slack-bot-reaction",
            "slack-agent-inbound",
            "slack-agent-reaction",
            "slack-agent-message-changed",
            "slack-agent-message-deleted",
            "slack-agent-thread-broadcast",
            "slack-agent-member",
            "slack-agent-channel",
            "slack-agent-app-home",
            "slack-agent-view-submission",
            "slack-bot-view-open",
            "slack-bot-view-publish",
            "slack-agent-view-closed",
            "slack-agent-pin",
            "slack-bot-set-status",
            "slack-bot-delete",
            "slack-bot-update",
            "slack-bot-upload",
            "slack-bot-suggested-prompts",
            "slack-bot-proactive",
            "slack-bot-ephemeral",
            "slack-bot-delete-file",
            "slack-agent-link-shared",
            "slack-bot-unfurl",
        ];
        let mut seen = std::collections::HashSet::new();
        for base in &bases {
            assert!(seen.insert(*base), "Duplicate consumer base name: {base}");
        }
    }

    #[test]
    fn consumer_name_with_account_id_all_consumers_unique() {
        // Same uniqueness guarantee when namespaced.
        let account_id = Some("myws");
        let bases = [
            "slack-bot-outbound",
            "slack-bot-stream-append",
            "slack-agent-inbound",
            "slack-agent-reaction",
        ];
        let mut seen = std::collections::HashSet::new();
        for base in &bases {
            let name = consumer_name(base, account_id);
            assert!(seen.insert(name.clone()), "Duplicate namespaced consumer name: {name}");
        }
    }
}
