//! Integration tests for OutboundProcessor — require a live NATS server.
//!
//! Run with: NATS_URL=nats://localhost:14222 cargo test outbound
//!
//! Tests are skipped automatically when NATS is unreachable.
//!
//! The fake Telegram API URL (http://127.0.0.1:19999/) causes instant
//! "Connection refused" errors, which the processor classifies and publishes
//! to the error subject. Tests verify receipt of those error events.

#[cfg(test)]
mod outbound_tests {
    use crate::outbound::{convert_chat_action, convert_parse_mode, OutboundProcessor};
    use futures::StreamExt;
    use telegram_nats::subjects;
    use telegram_types::commands::*;
    use teloxide::Bot;

    const DEFAULT_NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<async_nats::Client> {
        let url =
            std::env::var("NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
        async_nats::connect(&url).await.ok()
    }

    /// Creates a bot that immediately fails all API calls (connection refused).
    /// This avoids internet access and keeps tests fast.
    fn fake_bot() -> Bot {
        Bot::new("1234567890:AAAAAAAAAAAAAAAAAAAaaaaaaaaa").set_api_url(
            reqwest::Url::parse("http://127.0.0.1:19999/").unwrap(),
        )
    }

    /// Starts an OutboundProcessor in the background and waits for it to subscribe.
    async fn start_processor(client: async_nats::Client, prefix: &str) {
        let processor = OutboundProcessor::new(fake_bot(), client, prefix.to_string());
        tokio::spawn(async move {
            let _ = processor.run().await;
        });
        // Allow processor time to establish all NATS subscriptions
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    /// Waits for a message on a subscription with a 5-second timeout.
    async fn recv_msg(sub: &mut async_nats::Subscriber) -> serde_json::Value {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sub.next(),
        )
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscriber closed unexpectedly");
        serde_json::from_slice(&raw.payload).unwrap()
    }

    /// Publishes a command as JSON to the given NATS subject.
    async fn publish_cmd<T: serde::Serialize>(
        client: &async_nats::Client,
        subject: &str,
        cmd: &T,
    ) {
        let payload = serde_json::to_vec(cmd).unwrap();
        client
            .publish(subject.to_string(), payload.into())
            .await
            .unwrap();
    }

    // ── Unit tests (no I/O) ───────────────────────────────────────────────────

    #[test]
    #[allow(deprecated)]
    fn test_convert_parse_mode_variants() {
        use teloxide::types::ParseMode as TgParseMode;

        assert!(matches!(
            convert_parse_mode(ParseMode::Markdown),
            TgParseMode::Markdown
        ));
        assert!(matches!(
            convert_parse_mode(ParseMode::MarkdownV2),
            TgParseMode::MarkdownV2
        ));
        assert!(matches!(
            convert_parse_mode(ParseMode::HTML),
            TgParseMode::Html
        ));
    }

    #[test]
    fn test_convert_chat_action_variants() {
        use teloxide::types::ChatAction as TgAction;

        assert!(matches!(
            convert_chat_action(ChatAction::Typing),
            TgAction::Typing
        ));
        assert!(matches!(
            convert_chat_action(ChatAction::UploadPhoto),
            TgAction::UploadPhoto
        ));
        assert!(matches!(
            convert_chat_action(ChatAction::RecordVideo),
            TgAction::RecordVideo
        ));
        assert!(matches!(
            convert_chat_action(ChatAction::UploadVideo),
            TgAction::UploadVideo
        ));
        assert!(matches!(
            convert_chat_action(ChatAction::RecordVoice),
            TgAction::RecordVoice
        ));
        assert!(matches!(
            convert_chat_action(ChatAction::UploadVoice),
            TgAction::UploadVoice
        ));
        assert!(matches!(
            convert_chat_action(ChatAction::UploadDocument),
            TgAction::UploadDocument
        ));
        assert!(matches!(
            convert_chat_action(ChatAction::FindLocation),
            TgAction::FindLocation
        ));
    }

    // ── Integration tests (NATS required) ────────────────────────────────────

    /// Helper: run a single-command outbound integration test.
    /// Starts processor, publishes `cmd` to `cmd_subject`, waits for error
    /// event on the error subject, and returns the parsed error payload.
    async fn outbound_roundtrip<T: serde::Serialize>(
        prefix: &str,
        cmd_subject: String,
        cmd: &T,
    ) -> Option<serde_json::Value> {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return None;
        };

        let error_subject = subjects::bot::command_error(prefix);

        // Subscribe to error subject BEFORE publishing command
        let mut err_sub = client.subscribe(error_subject).await.unwrap();

        // Start processor in background
        start_processor(client.clone(), prefix).await;

        // Publish command
        publish_cmd(&client, &cmd_subject, cmd).await;

        // Receive error event (processor tried Telegram API → failed → published error)
        Some(recv_msg(&mut err_sub).await)
    }

    #[tokio::test]
    async fn test_outbound_send_message() {
        let prefix = format!("ob-send-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send(&prefix);
        let cmd = SendMessageCommand::new(99999_i64, "outbound test");

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(
            evt["command_subject"], cmd_subject,
            "error event should reference the command subject"
        );
    }

    #[tokio::test]
    async fn test_outbound_edit_message() {
        let prefix = format!("ob-edit-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_edit(&prefix);
        let cmd = EditMessageCommand {
            chat_id: 99999,
            message_id: 1,
            text: "edited".to_string(),
            parse_mode: None,
            reply_markup: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_delete_message() {
        let prefix = format!("ob-del-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_delete(&prefix);
        let cmd = DeleteMessageCommand {
            chat_id: 99999,
            message_id: 42,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_photo() {
        let prefix = format!("ob-photo-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_photo(&prefix);
        let cmd = SendPhotoCommand {
            chat_id: 99999,
            photo: "AgACAgIAAxkBAAIC".to_string(),
            caption: Some("test caption".to_string()),
            parse_mode: None,
            reply_to_message_id: None,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_poll() {
        let prefix = format!("ob-poll-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::poll_send(&prefix);
        let cmd = SendPollCommand {
            chat_id: 99999,
            question: "Which option?".to_string(),
            options: vec!["A".to_string(), "B".to_string()],
            is_anonymous: Some(true),
            poll_type: Some(PollKind::Regular),
            allows_multiple_answers: None,
            correct_option_id: None,
            explanation: None,
            explanation_parse_mode: None,
            open_period: None,
            close_date: None,
            is_closed: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_answer_callback() {
        let prefix = format!("ob-cb-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::callback_answer(&prefix);
        let cmd = AnswerCallbackCommand {
            callback_query_id: "cq_test_123".to_string(),
            text: Some("Done!".to_string()),
            show_alert: Some(false),
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_chat_action() {
        let prefix = format!("ob-action-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::chat_action(&prefix);
        let cmd = SendChatActionCommand {
            chat_id: 99999,
            action: ChatAction::Typing,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_location() {
        let prefix = format!("ob-loc-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_location(&prefix);
        let cmd = SendLocationCommand {
            chat_id: 99999,
            latitude: 48.8566,
            longitude: 2.3522,
            live_period: None,
            horizontal_accuracy: None,
            heading: None,
            proximity_alert_radius: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_venue() {
        let prefix = format!("ob-venue-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_venue(&prefix);
        let cmd = SendVenueCommand {
            chat_id: 99999,
            latitude: 48.8566,
            longitude: 2.3522,
            title: "Eiffel Tower".to_string(),
            address: "Champ de Mars".to_string(),
            foursquare_id: None,
            foursquare_type: None,
            google_place_id: None,
            google_place_type: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_contact() {
        let prefix = format!("ob-contact-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_contact(&prefix);
        let cmd = SendContactCommand {
            chat_id: 99999,
            phone_number: "+1234567890".to_string(),
            first_name: "Test".to_string(),
            last_name: None,
            vcard: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_video() {
        let prefix = format!("ob-video-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_video(&prefix);
        let cmd = SendVideoCommand {
            chat_id: 99999,
            video: "BAACAgIAAxkBAAI".to_string(),
            caption: None,
            parse_mode: None,
            duration: Some(10),
            width: Some(1280),
            height: Some(720),
            supports_streaming: Some(true),
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_document() {
        let prefix = format!("ob-doc-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_document(&prefix);
        let cmd = SendDocumentCommand {
            chat_id: 99999,
            document: "BQACAgIAAxkBAAI".to_string(),
            caption: Some("See attached".to_string()),
            parse_mode: None,
            reply_to_message_id: None,
            reply_markup: None,
            message_thread_id: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forward_message() {
        let prefix = format!("ob-fwd-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_forward(&prefix);
        let cmd = ForwardMessageCommand {
            chat_id: 99999,
            from_chat_id: 11111,
            message_id: 7,
            message_thread_id: None,
            disable_notification: None,
            protect_content: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_copy_message() {
        let prefix = format!("ob-copy-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_copy(&prefix);
        let cmd = CopyMessageCommand {
            chat_id: 99999,
            from_chat_id: 11111,
            message_id: 7,
            message_thread_id: None,
            caption: None,
            parse_mode: None,
            reply_to_message_id: None,
            reply_markup: None,
            disable_notification: None,
            protect_content: None,
        };

        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };

        assert_eq!(evt["command_subject"], cmd_subject);
    }

    /// Publishing invalid JSON to an agent subject should NOT crash the processor.
    /// After the bad message, a valid command should still be processed.
    #[tokio::test]
    async fn test_outbound_invalid_json_does_not_crash_processor() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };

        let prefix = format!("ob-invalid-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);

        let mut err_sub = client.subscribe(error_subject).await.unwrap();
        start_processor(client.clone(), &prefix).await;

        // 1. Publish garbage — processor logs error but keeps running
        client
            .publish(cmd_subject.clone(), b"NOT VALID JSON".as_ref().into())
            .await
            .unwrap();

        // 2. Short pause then publish a valid command
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cmd = SendMessageCommand::new(99999_i64, "after bad json");
        publish_cmd(&client, &cmd_subject, &cmd).await;

        // Processor is still alive — error event arrives for the valid command
        let evt = recv_msg(&mut err_sub).await;
        assert_eq!(
            evt["command_subject"], cmd_subject,
            "processor should continue processing after bad JSON"
        );
    }
}
