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
mod tests {
    use crate::outbound::{convert_chat_action, convert_parse_mode, OutboundProcessor};
    use futures::StreamExt;
    use telegram_nats::subjects;
    use telegram_types::commands::*;
    use teloxide::Bot;

    const DEFAULT_NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<async_nats::Client> {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
        async_nats::connect(&url).await.ok()
    }

    /// Creates a bot that immediately fails all API calls (connection refused).
    /// This avoids internet access and keeps tests fast.
    fn fake_bot() -> Bot {
        Bot::new("1234567890:AAAAAAAAAAAAAAAAAAAaaaaaaaaa")
            .set_api_url(reqwest::Url::parse("http://127.0.0.1:19999/").unwrap())
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
        let raw = tokio::time::timeout(std::time::Duration::from_secs(5), sub.next())
            .await
            .expect("timed out waiting for NATS message")
            .expect("subscriber closed unexpectedly");
        serde_json::from_slice(&raw.payload).unwrap()
    }

    /// Publishes a command as JSON to the given NATS subject.
    async fn publish_cmd<T: serde::Serialize>(client: &async_nats::Client, subject: &str, cmd: &T) {
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

    // ── Media (remaining) ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_outbound_stop_poll() {
        let prefix = format!("ob-stoppoll-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::poll_stop(&prefix);
        let cmd = StopPollCommand {
            chat_id: 99999,
            message_id: 7,
            reply_markup: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_audio() {
        let prefix = format!("ob-audio-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_audio(&prefix);
        let cmd = SendAudioCommand {
            chat_id: 99999,
            audio: "CQACAgIAAxk".to_string(),
            caption: None,
            parse_mode: None,
            duration: Some(120),
            performer: Some("Artist".to_string()),
            title: Some("Song".to_string()),
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
    async fn test_outbound_send_voice() {
        let prefix = format!("ob-voice-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_voice(&prefix);
        let cmd = SendVoiceCommand {
            chat_id: 99999,
            voice: "AwACAgIAAxk".to_string(),
            caption: None,
            parse_mode: None,
            duration: Some(10),
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
    async fn test_outbound_send_sticker() {
        let prefix = format!("ob-sticker-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_sticker(&prefix);
        let cmd = SendStickerCommand {
            chat_id: 99999,
            sticker: "CAACAgIAAxk".to_string(),
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
    async fn test_outbound_send_animation() {
        let prefix = format!("ob-anim-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_animation(&prefix);
        let cmd = SendAnimationCommand {
            chat_id: 99999,
            animation: "CgACAgIAAxk".to_string(),
            caption: None,
            parse_mode: None,
            duration: None,
            width: None,
            height: None,
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
    async fn test_outbound_send_video_note() {
        let prefix = format!("ob-vidnote-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_video_note(&prefix);
        let cmd = SendVideoNoteCommand {
            chat_id: 99999,
            video_note: "DQACAgIAAxk".to_string(),
            duration: Some(5),
            length: Some(240),
            reply_to_message_id: None,
            message_thread_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_send_media_group() {
        let prefix = format!("ob-mediagrp-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_send_media_group(&prefix);
        let cmd = SendMediaGroupCommand {
            chat_id: 99999,
            media: vec![
                InputMediaItem::Photo {
                    media: "p1".to_string(),
                    caption: None,
                    parse_mode: None,
                },
                InputMediaItem::Photo {
                    media: "p2".to_string(),
                    caption: None,
                    parse_mode: None,
                },
            ],
            reply_to_message_id: None,
            message_thread_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    /// The stream handler retries Telegram calls internally and does NOT publish
    /// to the error subject on failure — it just logs. Test that the command
    /// is correctly routed to the right NATS subject and can be deserialized.
    #[tokio::test]
    async fn test_outbound_stream_message() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("ob-stream-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::message_stream(&prefix);

        // Subscribe to the command subject from the test side (fan-out: test + processor both receive)
        let mut sub = client.subscribe(cmd_subject.clone()).await.unwrap();
        start_processor(client.clone(), &prefix).await;

        let cmd = StreamMessageCommand {
            chat_id: 99999,
            message_id: None,
            text: "Streaming chunk...".to_string(),
            parse_mode: None,
            is_final: false,
            session_id: Some("sess-1".to_string()),
            message_thread_id: None,
        };
        publish_cmd(&client, &cmd_subject, &cmd).await;

        // Verify the command arrived on the expected subject with correct content
        let msg = recv_msg(&mut sub).await;
        assert_eq!(msg["chat_id"], 99999);
        assert_eq!(msg["text"], "Streaming chunk...");
        assert_eq!(msg["session_id"], "sess-1");
    }

    #[tokio::test]
    async fn test_outbound_answer_inline_query() {
        let prefix = format!("ob-inline-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::inline_answer(&prefix);
        let cmd = AnswerInlineQueryCommand {
            inline_query_id: "iq_test_456".to_string(),
            results: vec![InlineQueryResult::Article(InlineQueryResultArticle {
                id: "r1".to_string(),
                title: "Result".to_string(),
                input_message_content: InputMessageContent {
                    message_text: "The answer".to_string(),
                    parse_mode: None,
                },
                description: None,
                thumb_url: None,
            })],
            cache_time: None,
            is_personal: None,
            next_offset: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    // ── Forum topics ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_outbound_forum_create() {
        let prefix = format!("ob-fcreate-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_create(&prefix);
        let cmd = CreateForumTopicCommand {
            chat_id: 99999,
            name: "Support".to_string(),
            icon_color: Some(0x6FB9F0),
            icon_custom_emoji_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_edit() {
        let prefix = format!("ob-fedit-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_edit(&prefix);
        let cmd = EditForumTopicCommand {
            chat_id: 99999,
            message_thread_id: 3,
            name: Some("Support v2".to_string()),
            icon_custom_emoji_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_close() {
        let prefix = format!("ob-fclose-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_close(&prefix);
        let cmd = CloseForumTopicCommand {
            chat_id: 99999,
            message_thread_id: 3,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_reopen() {
        let prefix = format!("ob-freopen-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_reopen(&prefix);
        let cmd = ReopenForumTopicCommand {
            chat_id: 99999,
            message_thread_id: 3,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_delete() {
        let prefix = format!("ob-fdel-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_delete(&prefix);
        let cmd = DeleteForumTopicCommand {
            chat_id: 99999,
            message_thread_id: 3,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_unpin() {
        let prefix = format!("ob-funpin-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_unpin(&prefix);
        let cmd = UnpinAllForumTopicMessagesCommand {
            chat_id: 99999,
            message_thread_id: 3,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_edit_general() {
        let prefix = format!("ob-fgeneral-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_edit_general(&prefix);
        let cmd = EditGeneralForumTopicCommand {
            chat_id: 99999,
            name: "General".to_string(),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_close_general() {
        let prefix = format!("ob-fclgen-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_close_general(&prefix);
        let cmd = CloseGeneralForumTopicCommand { chat_id: 99999 };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_reopen_general() {
        let prefix = format!("ob-freopgen-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_reopen_general(&prefix);
        let cmd = ReopenGeneralForumTopicCommand { chat_id: 99999 };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_hide_general() {
        let prefix = format!("ob-fhidegen-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_hide_general(&prefix);
        let cmd = HideGeneralForumTopicCommand { chat_id: 99999 };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_unhide_general() {
        let prefix = format!("ob-funhidegen-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_unhide_general(&prefix);
        let cmd = UnhideGeneralForumTopicCommand { chat_id: 99999 };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_forum_unpin_general() {
        let prefix = format!("ob-funpingen-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::forum_unpin_general(&prefix);
        let cmd = UnpinAllGeneralForumTopicMessagesCommand { chat_id: 99999 };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    // ── Admin commands ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_outbound_admin_promote() {
        use telegram_types::chat::ChatAdministratorRights;
        let prefix = format!("ob-promote-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_promote(&prefix);
        let cmd = PromoteChatMemberCommand {
            chat_id: 99999,
            user_id: 1001,
            rights: Some(ChatAdministratorRights {
                can_delete_messages: Some(true),
                ..Default::default()
            }),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_restrict() {
        use telegram_types::chat::ChatPermissions;
        let prefix = format!("ob-restrict-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_restrict(&prefix);
        let cmd = RestrictChatMemberCommand {
            chat_id: 99999,
            user_id: 1001,
            permissions: ChatPermissions {
                can_send_messages: Some(false),
                ..Default::default()
            },
            until_date: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_ban() {
        let prefix = format!("ob-ban-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_ban(&prefix);
        let cmd = BanChatMemberCommand {
            chat_id: 99999,
            user_id: 1001,
            until_date: None,
            revoke_messages: Some(true),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_unban() {
        let prefix = format!("ob-unban-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_unban(&prefix);
        let cmd = UnbanChatMemberCommand {
            chat_id: 99999,
            user_id: 1001,
            only_if_banned: Some(true),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_set_permissions() {
        use telegram_types::chat::ChatPermissions;
        let prefix = format!("ob-setperms-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_set_permissions(&prefix);
        let cmd = SetChatPermissionsCommand {
            chat_id: 99999,
            permissions: ChatPermissions {
                can_send_messages: Some(true),
                ..Default::default()
            },
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_set_title() {
        let prefix = format!("ob-settitle-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_set_title(&prefix);
        let cmd = SetChatAdministratorCustomTitleCommand {
            chat_id: 99999,
            user_id: 1001,
            custom_title: "Head of Ops".to_string(),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_pin() {
        let prefix = format!("ob-pin-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_pin(&prefix);
        let cmd = PinChatMessageCommand {
            chat_id: 99999,
            message_id: 42,
            disable_notification: Some(true),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_unpin() {
        let prefix = format!("ob-unpin-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_unpin(&prefix);
        let cmd = UnpinChatMessageCommand {
            chat_id: 99999,
            message_id: Some(42),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_unpin_all() {
        let prefix = format!("ob-unpinall-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_unpin_all(&prefix);
        let cmd = UnpinAllChatMessagesCommand { chat_id: 99999 };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_set_chat_title() {
        let prefix = format!("ob-chattitle-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_set_chat_title(&prefix);
        let cmd = SetChatTitleCommand {
            chat_id: 99999,
            title: "New Name".to_string(),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_admin_set_chat_description() {
        let prefix = format!("ob-chatdesc-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::admin_set_chat_description(&prefix);
        let cmd = SetChatDescriptionCommand {
            chat_id: 99999,
            description: "A test group".to_string(),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    // ── File commands ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_outbound_file_get() {
        let prefix = format!("ob-fileget-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::file_get(&prefix);
        let cmd = GetFileCommand {
            file_id: "AgACAgIAAxk".to_string(),
            request_id: Some("req-1".to_string()),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_file_download() {
        let prefix = format!("ob-filedl-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::file_download(&prefix);
        let cmd = DownloadFileCommand {
            file_id: "AgACAgIAAxk".to_string(),
            destination_path: "/tmp/test_file.jpg".to_string(),
            request_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    // ── Payment commands ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_outbound_payment_send_invoice() {
        use telegram_types::chat::LabeledPrice;
        let prefix = format!("ob-invoice-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::payment_send_invoice(&prefix);
        let cmd = SendInvoiceCommand {
            chat_id: 99999,
            title: "Pro Plan".to_string(),
            description: "Monthly subscription".to_string(),
            payload: "sub_monthly".to_string(),
            provider_token: "FAKE_PROVIDER_TOKEN".to_string(),
            currency: "USD".to_string(),
            prices: vec![LabeledPrice {
                label: "Monthly".to_string(),
                amount: 999,
            }],
            max_tip_amount: None,
            suggested_tip_amounts: None,
            start_parameter: None,
            provider_data: None,
            photo_url: None,
            photo_size: None,
            photo_width: None,
            photo_height: None,
            need_name: None,
            need_phone_number: None,
            need_email: None,
            need_shipping_address: None,
            send_email_to_provider: None,
            send_phone_number_to_provider: None,
            is_flexible: None,
            disable_notification: None,
            protect_content: None,
            reply_to_message_id: None,
            reply_markup: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_payment_answer_pre_checkout() {
        let prefix = format!("ob-precheckout-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::payment_answer_pre_checkout(&prefix);
        let cmd = AnswerPreCheckoutQueryCommand {
            pre_checkout_query_id: "pcq_abc123".to_string(),
            ok: true,
            error_message: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_payment_answer_shipping() {
        use telegram_types::chat::{LabeledPrice, ShippingOption};
        let prefix = format!("ob-shipping-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::payment_answer_shipping(&prefix);
        let cmd = AnswerShippingQueryCommand {
            shipping_query_id: "sq_xyz789".to_string(),
            ok: true,
            shipping_options: Some(vec![ShippingOption {
                id: "standard".to_string(),
                title: "Standard Shipping".to_string(),
                prices: vec![LabeledPrice {
                    label: "Shipping".to_string(),
                    amount: 500,
                }],
            }]),
            error_message: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    // ── Bot commands ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_outbound_bot_commands_set() {
        use telegram_types::chat::BotCommand;
        let prefix = format!("ob-cmdset-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::bot_commands_set(&prefix);
        let cmd = SetMyCommandsCommand {
            commands: vec![
                BotCommand {
                    command: "start".to_string(),
                    description: "Start the bot".to_string(),
                },
                BotCommand {
                    command: "help".to_string(),
                    description: "Show help".to_string(),
                },
            ],
            scope: None,
            language_code: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_bot_commands_delete() {
        let prefix = format!("ob-cmddel-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::bot_commands_delete(&prefix);
        let cmd = DeleteMyCommandsCommand {
            scope: None,
            language_code: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_bot_commands_get() {
        let prefix = format!("ob-cmdget-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::bot_commands_get(&prefix);
        let cmd = GetMyCommandsCommand {
            scope: None,
            language_code: None,
            request_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    // ── Sticker management ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_outbound_sticker_get_set() {
        let prefix = format!("ob-stkgetset-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_get_set(&prefix);
        let cmd = GetStickerSetCommand {
            name: "my_stickers".to_string(),
            request_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_upload_file() {
        let prefix = format!("ob-stkupload-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_upload_file(&prefix);
        let cmd = UploadStickerFileCommand {
            user_id: 1001,
            sticker: "CAACAgIAAxk".to_string(),
            format: "static".to_string(),
            request_id: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_create_set() {
        use telegram_types::chat::InputSticker;
        let prefix = format!("ob-stkcreateset-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_create_set(&prefix);
        let cmd = CreateNewStickerSetCommand {
            user_id: 1001,
            name: "my_pack_by_testbot".to_string(),
            title: "My Pack".to_string(),
            stickers: vec![InputSticker {
                sticker: "CAACAgIAAxk".to_string(),
                format: "static".to_string(),
                emoji_list: vec!["😀".to_string()],
                mask_position: None,
                keywords: vec![],
            }],
            sticker_type: None,
            needs_repainting: None,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_add_to_set() {
        use telegram_types::chat::InputSticker;
        let prefix = format!("ob-stkaddset-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_add_to_set(&prefix);
        let cmd = AddStickerToSetCommand {
            user_id: 1001,
            name: "my_pack_by_testbot".to_string(),
            sticker: InputSticker {
                sticker: "CAACAgIAAxk".to_string(),
                format: "static".to_string(),
                emoji_list: vec!["🎉".to_string()],
                mask_position: None,
                keywords: vec![],
            },
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_set_position() {
        let prefix = format!("ob-stkpos-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_set_position(&prefix);
        let cmd = SetStickerPositionInSetCommand {
            sticker: "CAACAgIAAxk".to_string(),
            position: 2,
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_delete_from_set() {
        let prefix = format!("ob-stkdelfromset-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_delete_from_set(&prefix);
        let cmd = DeleteStickerFromSetCommand {
            sticker: "CAACAgIAAxk".to_string(),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_set_title() {
        let prefix = format!("ob-stktitle-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_set_title(&prefix);
        let cmd = SetStickerSetTitleCommand {
            name: "my_pack_by_testbot".to_string(),
            title: "Better Pack".to_string(),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_set_thumbnail() {
        let prefix = format!("ob-stkthumb-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_set_thumbnail(&prefix);
        let cmd = SetStickerSetThumbnailCommand {
            name: "my_pack_by_testbot".to_string(),
            user_id: 1001,
            format: "static".to_string(),
            thumbnail: Some("CAACAgIAAxk".to_string()),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_delete_set() {
        let prefix = format!("ob-stkdelset-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_delete_set(&prefix);
        let cmd = DeleteStickerSetCommand {
            name: "my_pack_by_testbot".to_string(),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_set_emoji_list() {
        let prefix = format!("ob-stkemoji-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_set_emoji_list(&prefix);
        let cmd = SetStickerEmojiListCommand {
            sticker: "CAACAgIAAxk".to_string(),
            emoji_list: vec!["😀".to_string(), "🎉".to_string()],
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_set_keywords() {
        let prefix = format!("ob-stkkeywords-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_set_keywords(&prefix);
        let cmd = SetStickerKeywordsCommand {
            sticker: "CAACAgIAAxk".to_string(),
            keywords: vec!["funny".to_string(), "happy".to_string()],
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }

    #[tokio::test]
    async fn test_outbound_sticker_set_mask_position() {
        use telegram_types::chat::{MaskPoint, MaskPosition};
        let prefix = format!("ob-stkmask-{}", uuid::Uuid::new_v4().simple());
        let cmd_subject = subjects::agent::sticker_set_mask_position(&prefix);
        let cmd = SetStickerMaskPositionCommand {
            sticker: "CAACAgIAAxk".to_string(),
            mask_position: Some(MaskPosition {
                point: MaskPoint::Eyes,
                x_shift: 0.1,
                y_shift: -0.1,
                scale: 1.0,
            }),
        };
        let Some(evt) = outbound_roundtrip(&prefix, cmd_subject.clone(), &cmd).await else {
            return;
        };
        assert_eq!(evt["command_subject"], cmd_subject);
    }
}
