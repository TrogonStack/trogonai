//! Integration tests for TelegramBridge — require a live NATS + JetStream.
//!
//! Run with: NATS_URL=nats://localhost:14222 cargo test bridge_nats
//!
//! Tests are skipped automatically when NATS is unreachable.

#[cfg(test)]
mod nats_tests {
    use crate::bridge::TelegramBridge;
    use crate::handlers;
    use crate::health::AppState;
    use async_nats::jetstream;
    use futures::StreamExt;
    use telegram_types::{
        policies::{DmPolicy, GroupPolicy},
        AccessConfig,
    };
    use teloxide::{
        types::{CallbackQuery, Message},
        Bot,
    };

    const DEFAULT_NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<(async_nats::Client, jetstream::Context)> {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
        match async_nats::connect(&url).await {
            Ok(client) => {
                let js = jetstream::new(client.clone());
                Some((client, js))
            }
            Err(_) => None,
        }
    }

    async fn make_bridge(
        client: async_nats::Client,
        js: jetstream::Context,
        prefix: &str,
    ) -> TelegramBridge {
        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .expect("create KV bucket");

        TelegramBridge::new(
            client,
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Open,
                group_policy: GroupPolicy::Allowlist,
                ..Default::default()
            },
            kv,
        )
    }

    fn msg(payload: serde_json::Value) -> Message {
        serde_json::from_value(payload).expect("construct Message from JSON")
    }

    fn private_base(user_id: u64) -> serde_json::Value {
        serde_json::json!({
            "message_id": 1,
            "date": 1_700_000_000i64,
            "chat": {"id": user_id as i64, "type": "private", "first_name": "U"},
            "from": {"id": user_id, "is_bot": false, "first_name": "U"}
        })
    }

    fn group_base(user_id: u64, chat_id: i64) -> serde_json::Value {
        serde_json::json!({
            "message_id": 1,
            "date": 1_700_000_000i64,
            "chat": {"id": chat_id, "type": "group", "title": "Test Group"},
            "from": {"id": user_id, "is_bot": false, "first_name": "U"}
        })
    }

    async fn recv(sub: &mut async_nats::Subscriber) -> serde_json::Value {
        let raw = tokio::time::timeout(std::time::Duration::from_secs(3), sub.next())
            .await
            .expect("timed out waiting for NATS message")
            .expect("subscriber closed");
        serde_json::from_slice(&raw.payload).unwrap()
    }

    // ── text ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_text_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "integ-text";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["text"] = serde_json::json!("Hello NATS");
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.text", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_text_message(&message, 1).await.unwrap();

        let v = recv(&mut sub).await;
        assert_eq!(v["text"], "Hello NATS");
        assert_eq!(v["message"]["from"]["id"], 1);
    }

    #[tokio::test]
    async fn test_nats_text_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-text-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99); // user 99 not in allowlist
        json["text"] = serde_json::json!("should not arrive");
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.text", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(result.is_err(), "denied text must NOT be published to NATS");
    }

    // ── photo ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_photo_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "integ-photo";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(2);
        json["photo"] = serde_json::json!([
            {"file_id": "ph1", "file_unique_id": "uph1", "width": 320, "height": 240, "file_size": 2000}
        ]);
        json["caption"] = serde_json::json!("nice shot");
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.photo", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_photo_message(&message, 2).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["caption"], "nice shot");
        assert_eq!(event["photo"][0]["file_id"], "ph1");
    }

    // ── video ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_video_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "integ-video";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(3);
        json["video"] = serde_json::json!({
            "file_id": "vid1", "file_unique_id": "uvid1",
            "width": 1920, "height": 1080, "duration": 15,
            "file_size": 8000, "mime_type": "video/mp4"
        });
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.video", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_video_message(&message, 3).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["video"]["file_id"], "vid1");
        assert_eq!(event["video"]["mime_type"], "video/mp4");
    }

    // ── audio ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_audio_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "integ-audio";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(4);
        json["audio"] = serde_json::json!({
            "file_id": "aud1", "file_unique_id": "uaud1",
            "duration": 180, "file_size": 3600,
            "file_name": "song.ogg", "mime_type": "audio/ogg"
        });
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.audio", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_audio_message(&message, 4).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["audio"]["file_id"], "aud1");
        assert_eq!(event["audio"]["file_name"], "song.ogg");
    }

    // ── document ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_document_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "integ-doc";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(5);
        json["document"] = serde_json::json!({
            "file_id": "doc1", "file_unique_id": "udoc1",
            "file_size": 512, "file_name": "notes.txt",
            "mime_type": "text/plain"
        });
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.document", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_document_message(&message, 5).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["document"]["file_id"], "doc1");
        assert_eq!(event["document"]["file_name"], "notes.txt");
    }

    // ── voice ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_voice_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "integ-voice";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(6);
        json["voice"] = serde_json::json!({
            "file_id": "voc1", "file_unique_id": "uvoc1",
            "duration": 5, "file_size": 400, "mime_type": "audio/ogg"
        });
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.voice", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_voice_message(&message, 6).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["voice"]["file_id"], "voc1");
        assert_eq!(event["voice"]["mime_type"], "audio/ogg");
        assert!(event["voice"]["file_name"].is_null());
    }

    // ── Core message type — caption and access denied gaps ───────────────────

    #[tokio::test]
    async fn test_nats_photo_no_caption_is_null() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-photo-nocap";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(2);
        json["photo"] = serde_json::json!([
            {"file_id": "ph3", "file_unique_id": "uph3", "width": 320, "height": 240, "file_size": 1000}
        ]);
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.photo", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_photo_message(&message, 20).await.unwrap();

        let event = recv(&mut sub).await;
        assert!(
            event["caption"].is_null(),
            "photo without caption must have null caption"
        );
        assert_eq!(event["photo"][0]["file_id"], "ph3");
    }

    #[tokio::test]
    async fn test_nats_photo_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-photo-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99);
        json["photo"] = serde_json::json!([
            {"file_id": "ph4", "file_unique_id": "uph4", "width": 100, "height": 100, "file_size": 500}
        ]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.photo", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_photo_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(result.is_err(), "denied photo must NOT be published");
    }

    #[tokio::test]
    async fn test_nats_video_with_caption() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-video-cap";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(3);
        json["video"] = serde_json::json!({
            "file_id": "vid2", "file_unique_id": "uvid2",
            "width": 1280, "height": 720, "duration": 10,
            "file_size": 5000, "mime_type": "video/mp4"
        });
        json["caption"] = serde_json::json!("watch this");
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.video", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_video_message(&message, 30).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["caption"], "watch this");
        assert_eq!(event["video"]["file_id"], "vid2");
    }

    #[tokio::test]
    async fn test_nats_video_no_caption_is_null() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-video-nocap";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(3);
        json["video"] = serde_json::json!({
            "file_id": "vid3", "file_unique_id": "uvid3",
            "width": 640, "height": 480, "duration": 5,
            "file_size": 2000, "mime_type": "video/mp4"
        });
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.video", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_video_message(&message, 31).await.unwrap();

        let event = recv(&mut sub).await;
        assert!(
            event["caption"].is_null(),
            "video without caption must have null caption"
        );
    }

    #[tokio::test]
    async fn test_nats_video_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-video-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99);
        json["video"] = serde_json::json!({
            "file_id": "vid4", "file_unique_id": "uvid4",
            "width": 640, "height": 480, "duration": 3,
            "file_size": 1000, "mime_type": "video/mp4"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.video", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_video_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(result.is_err(), "denied video must NOT be published");
    }

    #[tokio::test]
    async fn test_nats_audio_with_caption_and_mime_type() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-audio-cap";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(4);
        json["audio"] = serde_json::json!({
            "file_id": "aud2", "file_unique_id": "uaud2",
            "duration": 210, "file_size": 4200,
            "file_name": "track.mp3", "mime_type": "audio/mpeg"
        });
        json["caption"] = serde_json::json!("great track");
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.audio", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_audio_message(&message, 40).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["caption"], "great track");
        assert_eq!(event["audio"]["mime_type"], "audio/mpeg");
        assert_eq!(event["audio"]["file_id"], "aud2");
    }

    #[tokio::test]
    async fn test_nats_audio_no_caption_is_null() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-audio-nocap";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(4);
        json["audio"] = serde_json::json!({
            "file_id": "aud3", "file_unique_id": "uaud3",
            "duration": 60, "file_size": 1200,
            "file_name": "clip.ogg", "mime_type": "audio/ogg"
        });
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.audio", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_audio_message(&message, 41).await.unwrap();

        let event = recv(&mut sub).await;
        assert!(
            event["caption"].is_null(),
            "audio without caption must have null caption"
        );
    }

    #[tokio::test]
    async fn test_nats_audio_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-audio-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99);
        json["audio"] = serde_json::json!({
            "file_id": "aud4", "file_unique_id": "uaud4",
            "duration": 30, "file_size": 600,
            "file_name": "blocked.ogg", "mime_type": "audio/ogg"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.audio", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_audio_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(result.is_err(), "denied audio must NOT be published");
    }

    #[tokio::test]
    async fn test_nats_document_with_caption_and_mime_type() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-doc-cap";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(5);
        json["document"] = serde_json::json!({
            "file_id": "doc2", "file_unique_id": "udoc2",
            "file_size": 2048, "file_name": "report.pdf",
            "mime_type": "application/pdf"
        });
        json["caption"] = serde_json::json!("Q3 report");
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.document", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_document_message(&message, 50).await.unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["caption"], "Q3 report");
        assert_eq!(event["document"]["mime_type"], "application/pdf");
        assert_eq!(event["document"]["file_id"], "doc2");
    }

    #[tokio::test]
    async fn test_nats_document_no_caption_is_null() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-doc-nocap";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(5);
        json["document"] = serde_json::json!({
            "file_id": "doc3", "file_unique_id": "udoc3",
            "file_size": 100, "file_name": "raw.bin",
            "mime_type": "application/octet-stream"
        });
        let message = msg(json);

        let subject = format!("telegram.{}.bot.message.document", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        bridge.publish_document_message(&message, 51).await.unwrap();

        let event = recv(&mut sub).await;
        assert!(
            event["caption"].is_null(),
            "document without caption must have null caption"
        );
    }

    #[tokio::test]
    async fn test_nats_document_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-doc-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99);
        json["document"] = serde_json::json!({
            "file_id": "doc4", "file_unique_id": "udoc4",
            "file_size": 50, "file_name": "blocked.txt",
            "mime_type": "text/plain"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.document", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_document_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(result.is_err(), "denied document must NOT be published");
    }

    #[tokio::test]
    async fn test_nats_voice_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-voice-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99);
        json["voice"] = serde_json::json!({
            "file_id": "voc2", "file_unique_id": "uvoc2",
            "duration": 3, "file_size": 200, "mime_type": "audio/ogg"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.voice", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_voice_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(result.is_err(), "denied voice must NOT be published");
    }

    // ── Handler routing ───────────────────────────────────────────────────────

    fn fake_bot() -> Bot {
        Bot::new("0000000000:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
    }

    fn health() -> AppState {
        AppState::new(Some("testbot".to_string()))
    }

    #[tokio::test]
    async fn test_handler_routes_text_to_nats() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "route-text";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(10);
        json["text"] = serde_json::json!("routed text");
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.text", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["text"], "routed text");
    }

    #[tokio::test]
    async fn test_handler_routes_photo_to_nats() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "route-photo";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(11);
        json["photo"] = serde_json::json!([
            {"file_id": "ph2", "file_unique_id": "uph2", "width": 800, "height": 600, "file_size": 3000}
        ]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.photo", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_photo_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["photo"][0]["file_id"], "ph2");
    }

    // ── Access denied blocks publish ──────────────────────────────────────────

    async fn make_bridge_restricted(
        client: async_nats::Client,
        js: jetstream::Context,
        prefix: &str,
    ) -> TelegramBridge {
        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .unwrap();

        TelegramBridge::new(
            client,
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Allowlist,
                user_allowlist: vec![], // nobody allowed
                ..Default::default()
            },
            kv,
        )
    }

    #[tokio::test]
    async fn test_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "deny-text";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99); // user 99 not in allowlist
        json["text"] = serde_json::json!("should not arrive");
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.text", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        // Nothing should arrive — wait 500ms then assert no message
        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(
            result.is_err(),
            "denied message must NOT be published to NATS"
        );
    }

    // ── Text with entities end-to-end ─────────────────────────────────────────

    #[tokio::test]
    async fn test_text_with_entities_published() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "entity-text";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(20);
        json["text"] = serde_json::json!("@alice hello #world");
        json["entities"] = serde_json::json!([
            {"type": "mention",  "offset": 0, "length": 6},
            {"type": "hashtag",  "offset": 13, "length": 6}
        ]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.text", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["text"], "@alice hello #world");
        let entities = event["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0]["type"], "mention");
        assert_eq!(entities[1]["type"], "hashtag");
    }

    // ── Commands ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_command_start_published() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "cmd-start";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(42);
        json["text"] = serde_json::json!("/start");
        json["entities"] = serde_json::json!([{"type": "bot_command", "offset": 0, "length": 6}]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.command.start", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["command"], "start");
        let args = event["args"].as_array().unwrap();
        assert!(args.is_empty());
    }

    #[tokio::test]
    async fn test_command_help_published() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "cmd-help";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(42);
        json["text"] = serde_json::json!("/help");
        json["entities"] = serde_json::json!([{"type": "bot_command", "offset": 0, "length": 5}]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.command.help", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["command"], "help");
    }

    #[tokio::test]
    async fn test_custom_command_with_args() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "cmd-custom";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(42);
        json["text"] = serde_json::json!("/search rust async");
        json["entities"] = serde_json::json!([{"type": "bot_command", "offset": 0, "length": 7}]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.command.search", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["command"], "search");
        let args = event["args"].as_array().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "rust");
        assert_eq!(args[1], "async");
    }

    #[tokio::test]
    async fn test_command_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "cmd-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99); // denied user
        json["text"] = serde_json::json!("/start");
        json["entities"] = serde_json::json!([{"type": "bot_command", "offset": 0, "length": 6}]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.command.start", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(
            result.is_err(),
            "denied command must NOT be published to NATS"
        );
    }

    #[tokio::test]
    async fn test_command_admin_bypasses_access_restriction() {
        // Admin users must be able to send commands even when DmPolicy is Allowlist
        // and the user is not in the allowlist.
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "cmd-admin";
        const ADMIN_ID: u64 = 1001;

        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .expect("create KV bucket");
        let bridge = TelegramBridge::new(
            client.clone(),
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Allowlist,
                user_allowlist: vec![],             // nobody in allowlist
                admin_users: vec![ADMIN_ID as i64], // but this user is admin
                ..Default::default()
            },
            kv,
        );

        let mut json = private_base(ADMIN_ID);
        json["text"] = serde_json::json!("/start");
        json["entities"] = serde_json::json!([{"type": "bot_command", "offset": 0, "length": 6}]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.command.start", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["command"], "start");
    }

    #[tokio::test]
    async fn test_command_from_group_with_bot_suffix() {
        // In group chats Telegram sends commands as "/start@botname".
        // The handler must strip the "@botname" and publish to "bot.command.start".
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "cmd-group";
        const GROUP_ID: i64 = -100123456789;

        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .expect("create KV bucket");
        let bridge = TelegramBridge::new(
            client.clone(),
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Open,
                group_policy: GroupPolicy::Allowlist,
                group_allowlist: vec![GROUP_ID],
                ..Default::default()
            },
            kv,
        );

        let mut json = group_base(42, GROUP_ID);
        json["text"] = serde_json::json!("/start@testbot");
        json["entities"] = serde_json::json!([{"type": "bot_command", "offset": 0, "length": 14}]);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.command.start", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_text_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["command"], "start");
        let args = event["args"].as_array().unwrap();
        assert!(args.is_empty());
    }

    // ── Callback queries ──────────────────────────────────────────────────────

    fn callback_query_json(
        user_id: u64,
        data: &str,
        message_id: i64,
        chat_id: i64,
    ) -> CallbackQuery {
        serde_json::from_value(serde_json::json!({
            "id": "123456789",
            "from": {"id": user_id, "is_bot": false, "first_name": "User"},
            "data": data,
            "chat_instance": "abc",
            "message": {
                "message_id": message_id,
                "date": 1_700_000_000i64,
                "chat": {"id": chat_id, "type": "private", "first_name": "User"},
                "from": {"id": user_id, "is_bot": false, "first_name": "User"},
                "text": "button text"
            }
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn test_handler_routes_callback_query_to_nats() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "route-callback";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let query = callback_query_json(42, "action:confirm", 1, 42);
        let subject = format!("telegram.{}.bot.callback.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_callback_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["data"], "action:confirm");
        assert_eq!(event["from"]["id"], 42);
        assert_eq!(event["callback_query_id"], "123456789");
    }

    #[tokio::test]
    async fn test_callback_query_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "deny-callback";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let query = callback_query_json(99, "action:deny", 1, 99);
        let subject = format!("telegram.{}.bot.callback.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_callback_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(
            result.is_err(),
            "denied callback must NOT be published to NATS"
        );
    }

    #[tokio::test]
    async fn test_callback_query_without_message_inline_mode() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "callback-inline";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        // Inline mode: no "message" field, only inline_message_id
        let query: CallbackQuery = serde_json::from_value(serde_json::json!({
            "id": "777",
            "from": {"id": 55, "is_bot": false, "first_name": "U"},
            "chat_instance": "inline_chat",
            "inline_message_id": "ABCDEF",
            "data": "inline:action"
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.callback.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_callback_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["data"], "inline:action");
        assert_eq!(event["from"]["id"], 55);
        // No message → message_id should be null
        assert!(event["message_id"].is_null());
        // chat.id should fall back to user_id
        assert_eq!(event["chat"]["id"], 55);
    }

    #[tokio::test]
    async fn test_callback_query_empty_data() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "callback-empty";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        // No "data" field → bridge should publish with empty string
        let query: CallbackQuery = serde_json::from_value(serde_json::json!({
            "id": "999",
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "chat_instance": "xyz",
            "message": {
                "message_id": 5,
                "date": 1_700_000_000i64,
                "chat": {"id": 42, "type": "private", "first_name": "U"},
                "from": {"id": 42, "is_bot": false, "first_name": "U"},
                "text": "x"
            }
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.callback.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_callback_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["data"], "");
    }

    #[tokio::test]
    async fn test_callback_query_message_id_forwarded() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "callback-msgid";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let query = callback_query_json(42, "click", 77, 42);
        let subject = format!("telegram.{}.bot.callback.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_callback_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["message_id"], 77);
        assert_eq!(event["data"], "click");
    }

    #[tokio::test]
    async fn test_callback_query_admin_bypasses_restriction() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "callback-admin";
        const ADMIN_ID: u64 = 3001;

        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .unwrap();
        let bridge = TelegramBridge::new(
            client.clone(),
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Allowlist,
                user_allowlist: vec![],
                admin_users: vec![ADMIN_ID as i64],
                ..Default::default()
            },
            kv,
        );

        let query = callback_query_json(ADMIN_ID, "admin:action", 1, ADMIN_ID as i64);
        let subject = format!("telegram.{}.bot.callback.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_callback_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["data"], "admin:action");
        assert_eq!(event["from"]["id"], ADMIN_ID);
    }

    // ── Inline queries ────────────────────────────────────────────────────────

    fn inline_query_json(user_id: u64, query: &str) -> teloxide::types::InlineQuery {
        serde_json::from_value(serde_json::json!({
            "id": "iq-1",
            "from": {"id": user_id, "is_bot": false, "first_name": "U"},
            "query": query,
            "offset": ""
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn test_inline_query_published() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "iq-basic";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let query = inline_query_json(42, "rust async");
        let subject = format!("telegram.{}.bot.inline.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_inline_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["query"], "rust async");
        assert_eq!(event["from"]["id"], 42);
        assert_eq!(event["inline_query_id"], "iq-1");
    }

    #[tokio::test]
    async fn test_inline_query_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "iq-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let query = inline_query_json(99, "should be denied");
        let subject = format!("telegram.{}.bot.inline.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_inline_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(
            result.is_err(),
            "denied inline query must NOT be published to NATS"
        );
    }

    #[tokio::test]
    async fn test_inline_query_admin_bypasses_restriction() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "iq-admin";
        const ADMIN_ID: u64 = 2001;

        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .expect("create KV bucket");
        let bridge = TelegramBridge::new(
            client.clone(),
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Allowlist,
                user_allowlist: vec![],
                admin_users: vec![ADMIN_ID as i64],
                ..Default::default()
            },
            kv,
        );

        let query = inline_query_json(ADMIN_ID, "admin query");
        let subject = format!("telegram.{}.bot.inline.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_inline_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["query"], "admin query");
    }

    #[tokio::test]
    async fn test_inline_query_empty_query_published() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "iq-empty";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        // User types just "@bot" with no search text
        let query = inline_query_json(42, "");
        let subject = format!("telegram.{}.bot.inline.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_inline_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["query"], "");
        assert_eq!(event["inline_query_id"], "iq-1");
    }

    #[tokio::test]
    async fn test_inline_query_offset_forwarded() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "iq-offset";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        // Pagination: Telegram sends offset="5" when user scrolls past first page
        let query: teloxide::types::InlineQuery = serde_json::from_value(serde_json::json!({
            "id": "iq-page2",
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "query": "rust",
            "offset": "5"
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.inline.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_inline_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["query"], "rust");
        assert_eq!(event["offset"], "5");
        assert_eq!(event["inline_query_id"], "iq-page2");
    }

    #[tokio::test]
    async fn test_inline_query_chat_type_forwarded() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "iq-chattype";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let query: teloxide::types::InlineQuery = serde_json::from_value(serde_json::json!({
            "id": "iq-ct",
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "query": "search",
            "offset": "",
            "chat_type": "private"
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.inline.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_inline_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["query"], "search");
        // chat_type is forwarded (teloxide formats it via Debug)
        assert!(
            !event["chat_type"].is_null(),
            "chat_type must be present when provided"
        );
    }

    #[tokio::test]
    async fn test_chosen_inline_result_with_inline_message_id() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "chosen-imid";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        // inline_message_id is present when the bot can edit the sent message later
        let result: teloxide::types::ChosenInlineResult =
            serde_json::from_value(serde_json::json!({
                "result_id": "res_xyz",
                "from": {"id": 42, "is_bot": false, "first_name": "U"},
                "query": "rust",
                "inline_message_id": "ABCDEF123"
            }))
            .unwrap();

        let subject = format!("telegram.{}.bot.inline.chosen", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_chosen_inline_result(fake_bot(), result, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["result_id"], "res_xyz");
        assert_eq!(event["query"], "rust");
        assert_eq!(event["inline_message_id"], "ABCDEF123");
    }

    #[tokio::test]
    async fn test_inline_query_location_forwarded() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = "iq-location";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        // Bot configured with supports_inline_queries + location request
        let query: teloxide::types::InlineQuery = serde_json::from_value(serde_json::json!({
            "id": "iq-loc",
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "query": "coffee near me",
            "offset": "",
            "location": {"longitude": -122.4194, "latitude": 37.7749}
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.inline.query", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        handlers::handle_inline_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["query"], "coffee near me");
        let loc = &event["location"];
        assert!(!loc.is_null(), "location must be present");
        assert!((loc["longitude"].as_f64().unwrap() - (-122.4194)).abs() < 0.001);
        assert!((loc["latitude"].as_f64().unwrap() - 37.7749).abs() < 0.001);
    }

    // ── Message types: location, venue, contact, sticker, animation, video_note

    #[tokio::test]
    async fn test_nats_location_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-location";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["location"] = serde_json::json!({"longitude": 2.3514, "latitude": 48.8566});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.location", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_location_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert!((event["longitude"].as_f64().unwrap() - 2.3514).abs() < 0.001);
        assert!((event["latitude"].as_f64().unwrap() - 48.8566).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_nats_venue_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-venue";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["venue"] = serde_json::json!({
            "location": {"longitude": 2.3514, "latitude": 48.8566},
            "title": "Eiffel Tower",
            "address": "Champ de Mars"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.venue", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_venue_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["title"], "Eiffel Tower");
        assert_eq!(event["address"], "Champ de Mars");
    }

    #[tokio::test]
    async fn test_nats_contact_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-contact";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["contact"] = serde_json::json!({"phone_number": "+1234567890", "first_name": "Alice"});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.contact", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_contact_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["phone_number"], "+1234567890");
        assert_eq!(event["first_name"], "Alice");
    }

    #[tokio::test]
    async fn test_nats_sticker_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-sticker";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["sticker"] = serde_json::json!({
            "file_id": "CAACAgIA", "file_unique_id": "AgAD01",
            "type": "regular", "width": 512, "height": 512,
            "is_animated": false, "is_video": false, "file_size": 12345
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.sticker", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_sticker_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["file_id"], "CAACAgIA");
        assert_eq!(event["kind"], "regular");
    }

    #[tokio::test]
    async fn test_nats_animation_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-animation";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        // Telegram sends animation messages with both "animation" and "document" fields.
        json["animation"] = serde_json::json!({
            "file_id": "CgACAgIA", "file_unique_id": "AgAD02",
            "width": 320, "height": 240, "duration": 3,
            "file_name": "anim.gif", "mime_type": "image/gif", "file_size": 50000
        });
        json["document"] = serde_json::json!({
            "file_id": "CgACAgIA", "file_unique_id": "AgAD02",
            "file_name": "anim.gif", "mime_type": "image/gif", "file_size": 50000
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.animation", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_animation_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["animation"]["file_id"], "CgACAgIA");
        assert_eq!(event["width"], 320);
        assert_eq!(event["height"], 240);
        assert_eq!(event["duration"], 3);
    }

    #[tokio::test]
    async fn test_nats_video_note_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-videonote";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["video_note"] = serde_json::json!({
            "file_id": "DQACAgIA", "file_unique_id": "AgAD03",
            "length": 240, "duration": 5, "file_size": 100000
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.video_note", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_video_note_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["video_note"]["file_id"], "DQACAgIA");
        assert_eq!(event["duration"], 5);
        assert_eq!(event["length"], 240);
    }

    // ── Sticker extra fields ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_sticker_emoji_and_set_name_forwarded() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-sticker-emoji";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["sticker"] = serde_json::json!({
            "file_id": "CAACAgIA", "file_unique_id": "AgAD01",
            "type": "regular", "width": 512, "height": 512,
            "is_animated": false, "is_video": false, "file_size": 12345,
            "emoji": "😀",
            "set_name": "MyPack"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.sticker", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_sticker_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["emoji"], "😀");
        assert_eq!(event["set_name"], "MyPack");
        assert_eq!(event["format"], "static");
    }

    #[tokio::test]
    async fn test_nats_sticker_format_animated() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-sticker-anim";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["sticker"] = serde_json::json!({
            "file_id": "CAACAgIA2", "file_unique_id": "AgAD02",
            "type": "regular", "width": 512, "height": 512,
            "is_animated": true, "is_video": false, "file_size": 8000
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.sticker", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_sticker_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["format"], "animated");
    }

    #[tokio::test]
    async fn test_nats_sticker_format_video() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-sticker-video";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["sticker"] = serde_json::json!({
            "file_id": "CAACAgIA3", "file_unique_id": "AgAD03",
            "type": "regular", "width": 512, "height": 512,
            "is_animated": false, "is_video": true, "file_size": 20000
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.sticker", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_sticker_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["format"], "video");
    }

    #[tokio::test]
    async fn test_nats_sticker_kind_mask() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-sticker-mask";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["sticker"] = serde_json::json!({
            "file_id": "CAACAgIA4", "file_unique_id": "AgAD04",
            "type": "mask", "width": 512, "height": 512,
            "is_animated": false, "is_video": false, "file_size": 12000,
            "mask_position": {"point": "forehead", "x_shift": 0.0, "y_shift": 0.0, "scale": 1.0}
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.sticker", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_sticker_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["kind"], "mask");
    }

    #[tokio::test]
    async fn test_nats_sticker_kind_custom_emoji() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-sticker-cemoji";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["sticker"] = serde_json::json!({
            "file_id": "CAACAgIA5", "file_unique_id": "AgAD05",
            "type": "custom_emoji", "width": 100, "height": 100,
            "is_animated": false, "is_video": false, "file_size": 5000,
            "custom_emoji_id": "5364471070641932931"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.sticker", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_sticker_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["kind"], "custom_emoji");
    }

    #[tokio::test]
    async fn test_nats_sticker_access_denied_does_not_publish() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-sticker-deny";
        let bridge = make_bridge_restricted(client.clone(), js, prefix).await;

        let mut json = private_base(99); // denied user
        json["sticker"] = serde_json::json!({
            "file_id": "CAACAgIA6", "file_unique_id": "AgAD06",
            "type": "regular", "width": 512, "height": 512,
            "is_animated": false, "is_video": false, "file_size": 12345
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.sticker", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_sticker_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        assert!(
            result.is_err(),
            "denied sticker must NOT be published to NATS"
        );
    }

    // ── Animation extra fields ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_animation_with_caption() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-anim-caption";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["animation"] = serde_json::json!({
            "file_id": "CgACAgIA2", "file_unique_id": "AgAD07",
            "width": 480, "height": 270, "duration": 5,
            "file_name": "funny.gif", "mime_type": "image/gif", "file_size": 30000
        });
        json["document"] = serde_json::json!({
            "file_id": "CgACAgIA2", "file_unique_id": "AgAD07",
            "file_name": "funny.gif", "mime_type": "image/gif", "file_size": 30000
        });
        json["caption"] = serde_json::json!("haha");
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.animation", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_animation_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["caption"], "haha");
        assert_eq!(event["animation"]["file_id"], "CgACAgIA2");
    }

    #[tokio::test]
    async fn test_nats_animation_no_caption_is_null() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-anim-nocaption";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["animation"] = serde_json::json!({
            "file_id": "CgACAgIA3", "file_unique_id": "AgAD08",
            "width": 320, "height": 240, "duration": 2,
            "file_name": "loop.gif", "mime_type": "image/gif", "file_size": 10000
        });
        json["document"] = serde_json::json!({
            "file_id": "CgACAgIA3", "file_unique_id": "AgAD08",
            "file_name": "loop.gif", "mime_type": "image/gif", "file_size": 10000
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.animation", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_animation_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert!(
            event["caption"].is_null(),
            "caption must be null when absent"
        );
    }

    // ── Chosen inline result ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_chosen_inline_result() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-chosen";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let result: teloxide::types::ChosenInlineResult =
            serde_json::from_value(serde_json::json!({
                "result_id": "result_abc",
                "from": {"id": 42, "is_bot": false, "first_name": "U"},
                "query": "rust async"
            }))
            .unwrap();

        let subject = format!("telegram.{}.bot.inline.chosen", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_chosen_inline_result(fake_bot(), result, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["result_id"], "result_abc");
        assert_eq!(event["query"], "rust async");
        assert_eq!(event["from"]["id"], 42);
    }

    // ── Chat member updates ──────────────────────────────────────────────────

    fn chat_member_updated_json(
        chat_id: i64,
        user_id: u64,
        old_status: &str,
        new_status: &str,
    ) -> teloxide::types::ChatMemberUpdated {
        serde_json::from_value(serde_json::json!({
            "chat": {"id": chat_id, "type": "group", "title": "Test Group"},
            "from": {"id": user_id, "is_bot": false, "first_name": "U"},
            "date": 1_700_000_000i64,
            "old_chat_member": {
                "user": {"id": user_id, "is_bot": false, "first_name": "U"},
                "status": old_status
            },
            "new_chat_member": {
                "user": {"id": user_id, "is_bot": false, "first_name": "U"},
                "status": new_status
            }
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn test_nats_chat_member_updated() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-chatmember";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let update = chat_member_updated_json(-100111, 42, "member", "left");

        let subject = format!("telegram.{}.bot.chat.member_updated", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_chat_member_updated(fake_bot(), update, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["new_chat_member"]["status"], "left");
        assert_eq!(event["from"]["id"], 42);
    }

    #[tokio::test]
    async fn test_nats_my_chat_member_updated() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-mychatmember";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        // Bot (id=1) added to a group
        let update = chat_member_updated_json(-100222, 1, "left", "member");

        let subject = format!("telegram.{}.bot.chat.my_member_updated", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_my_chat_member_updated(fake_bot(), update, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["new_chat_member"]["status"], "member");
    }

    // ── Payments ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_pre_checkout_query() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-precheckout";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let query: teloxide::types::PreCheckoutQuery = serde_json::from_value(serde_json::json!({
            "id": "pcq_1",
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "currency": "USD",
            "total_amount": 1000,
            "invoice_payload": "order_123"
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.payment.pre_checkout", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_pre_checkout_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["currency"], "USD");
        assert_eq!(event["total_amount"], 1000);
        assert_eq!(event["invoice_payload"], "order_123");
    }

    #[tokio::test]
    async fn test_nats_shipping_query() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-shipping";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let query: teloxide::types::ShippingQuery = serde_json::from_value(serde_json::json!({
            "id": "sq_1",
            "from": {"id": 42, "is_bot": false, "first_name": "U"},
            "invoice_payload": "order_456",
            "shipping_address": {
                "country_code": "US", "state": "CA",
                "city": "San Francisco", "street_line1": "123 Main St",
                "street_line2": "", "post_code": "94105"
            }
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.payment.shipping", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_shipping_query(fake_bot(), query, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["invoice_payload"], "order_456");
        assert_eq!(event["shipping_address"]["city"], "San Francisco");
    }

    #[tokio::test]
    async fn test_nats_successful_payment() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-payment";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(42);
        json["successful_payment"] = serde_json::json!({
            "currency": "USD", "total_amount": 2500,
            "invoice_payload": "order_789",
            "telegram_payment_charge_id": "charge_abc",
            "provider_payment_charge_id": "provider_xyz"
        });
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.payment.successful", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_successful_payment(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["payment"]["currency"], "USD");
        assert_eq!(event["payment"]["total_amount"], 2500);
    }

    // ── Poll ─────────────────────────────────────────────────────────────────

    fn poll_json() -> serde_json::Value {
        serde_json::json!({
            "id": "poll_1",
            "question": "Favorite language?",
            "options": [
                {"text": "Rust", "voter_count": 10},
                {"text": "Python", "voter_count": 5}
            ],
            "total_voter_count": 15, "is_closed": false,
            "is_anonymous": true, "type": "regular",
            "allows_multiple_answers": false
        })
    }

    #[tokio::test]
    async fn test_nats_poll_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-pollmsg";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(1);
        json["poll"] = poll_json();
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.poll", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_poll_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["question"], "Favorite language?");
        assert_eq!(event["total_voter_count"], 15);
    }

    #[tokio::test]
    async fn test_nats_poll_update() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-pollupd";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let poll: teloxide::types::Poll = serde_json::from_value(poll_json()).unwrap();

        let subject = format!("telegram.{}.bot.poll.update", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_poll_update(fake_bot(), poll, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["poll_id"], "poll_1");
        assert_eq!(event["question"], "Favorite language?");
    }

    #[tokio::test]
    async fn test_nats_poll_answer() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-pollanswer";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let answer: teloxide::types::PollAnswer = serde_json::from_value(serde_json::json!({
            "poll_id": "poll_1",
            "user": {"id": 42, "is_bot": false, "first_name": "U"},
            "option_ids": [0]
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.poll.answer", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_poll_answer(fake_bot(), answer, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["poll_id"], "poll_1");
        assert_eq!(event["option_ids"][0], 0);
        assert_eq!(event["voter_user_id"], 42);
    }

    // ── Edited message, channel post, edited channel post ────────────────────

    #[tokio::test]
    async fn test_nats_edited_message() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-edited";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = private_base(42);
        json["text"] = serde_json::json!("edited text");
        json["edit_date"] = serde_json::json!(1_700_001_000i64);
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.message.edited", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_edited_message(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["new_text"], "edited text");
    }

    #[tokio::test]
    async fn test_nats_channel_post() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-chanpost";
        const CHANNEL_ID: i64 = -100999888;

        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .expect("create KV");
        let bridge = TelegramBridge::new(
            client.clone(),
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Open,
                group_policy: GroupPolicy::Allowlist,
                group_allowlist: vec![CHANNEL_ID],
                ..Default::default()
            },
            kv,
        );

        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 1, "date": 1_700_000_000i64,
            "chat": {"id": CHANNEL_ID, "type": "channel", "title": "My Channel"},
            "text": "Hello channel"
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.message.text", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_channel_post(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["text"], "Hello channel");
    }

    async fn make_channel_bridge(
        client: async_nats::Client,
        js: jetstream::Context,
        prefix: &str,
        channel_id: i64,
    ) -> TelegramBridge {
        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .expect("create KV");
        TelegramBridge::new(
            client,
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Open,
                group_policy: GroupPolicy::Allowlist,
                group_allowlist: vec![channel_id],
                ..Default::default()
            },
            kv,
        )
    }

    #[tokio::test]
    async fn test_nats_channel_post_photo() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-chanpost-photo";
        const CHANNEL_ID: i64 = -100111222;
        let bridge = make_channel_bridge(client.clone(), js, prefix, CHANNEL_ID).await;

        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 2, "date": 1_700_000_000i64,
            "chat": {"id": CHANNEL_ID, "type": "channel", "title": "My Channel"},
            "photo": [{"file_id": "ph1", "file_unique_id": "uph1", "width": 320, "height": 240, "file_size": 2000}]
        })).unwrap();

        let subject = format!("telegram.{}.bot.message.photo", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_channel_post(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert!(!event["photo"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_nats_channel_post_video() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-chanpost-video";
        const CHANNEL_ID: i64 = -100333444;
        let bridge = make_channel_bridge(client.clone(), js, prefix, CHANNEL_ID).await;

        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 3, "date": 1_700_000_000i64,
            "chat": {"id": CHANNEL_ID, "type": "channel", "title": "My Channel"},
            "video": {
                "file_id": "vid1", "file_unique_id": "uvid1",
                "width": 1280, "height": 720, "duration": 10,
                "file_size": 8000, "mime_type": "video/mp4"
            }
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.message.video", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_channel_post(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["video"]["file_id"], "vid1");
    }

    #[tokio::test]
    async fn test_nats_channel_post_document() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-chanpost-doc";
        const CHANNEL_ID: i64 = -100555666;
        let bridge = make_channel_bridge(client.clone(), js, prefix, CHANNEL_ID).await;

        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 4, "date": 1_700_000_000i64,
            "chat": {"id": CHANNEL_ID, "type": "channel", "title": "My Channel"},
            "document": {"file_id": "doc1", "file_unique_id": "udoc1"}
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.message.document", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_channel_post(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["document"]["file_id"], "doc1");
    }

    #[tokio::test]
    async fn test_nats_edited_channel_post() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-chanedited";
        const CHANNEL_ID: i64 = -100777666;

        let bucket = format!("sessions-{}", uuid::Uuid::new_v4().simple());
        let kv = js
            .create_key_value(jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .expect("create KV");
        let bridge = TelegramBridge::new(
            client.clone(),
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Open,
                group_policy: GroupPolicy::Allowlist,
                group_allowlist: vec![CHANNEL_ID],
                ..Default::default()
            },
            kv,
        );

        let message: Message = serde_json::from_value(serde_json::json!({
            "message_id": 2, "date": 1_700_000_000i64,
            "edit_date": 1_700_001_000i64,
            "chat": {"id": CHANNEL_ID, "type": "channel", "title": "My Channel"},
            "text": "edited channel text"
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.message.edited", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_edited_channel_post(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["new_text"], "edited channel text");
    }

    // ── Chat join request ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_nats_chat_join_request() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-joinreq";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let request: teloxide::types::ChatJoinRequest = serde_json::from_value(serde_json::json!({
            "chat": {"id": -100333, "type": "group", "title": "VIP Group"},
            "from": {"id": 42, "is_bot": false, "first_name": "Alice"},
            "user_chat_id": 42,
            "date": 1_700_000_000i64
        }))
        .unwrap();

        let subject = format!("telegram.{}.bot.chat.join_request", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_chat_join_request(fake_bot(), request, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["from"]["id"], 42);
        assert_eq!(event["from"]["first_name"], "Alice");
    }

    // ── Forum topics ─────────────────────────────────────────────────────────

    fn forum_msg_base(user_id: u64) -> serde_json::Value {
        serde_json::json!({
            "message_id": 1,
            "date": 1_700_000_000i64,
            "chat": {"id": user_id as i64, "type": "private", "first_name": "U"},
            "from": {"id": user_id, "is_bot": false, "first_name": "U"}
        })
    }

    #[tokio::test]
    async fn test_nats_forum_topic_created() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-forum-created";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = forum_msg_base(42);
        json["forum_topic_created"] = serde_json::json!({"name": "Help", "icon_color": 7322096});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.forum.created", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_forum_topic_created(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["forum_topic"]["name"], "Help");
    }

    #[tokio::test]
    async fn test_nats_forum_topic_edited() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-forum-edited";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = forum_msg_base(42);
        json["forum_topic_edited"] = serde_json::json!({"name": "Help v2"});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.forum.edited", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_forum_topic_edited(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        let event = recv(&mut sub).await;
        assert_eq!(event["name"], "Help v2");
    }

    #[tokio::test]
    async fn test_nats_forum_topic_closed() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-forum-closed";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = forum_msg_base(42);
        json["forum_topic_closed"] = serde_json::json!({});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.forum.closed", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_forum_topic_closed(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        recv(&mut sub).await; // just verify it published
    }

    #[tokio::test]
    async fn test_nats_forum_topic_reopened() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-forum-reopened";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = forum_msg_base(42);
        json["forum_topic_reopened"] = serde_json::json!({});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.forum.reopened", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_forum_topic_reopened(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        recv(&mut sub).await;
    }

    #[tokio::test]
    async fn test_nats_general_forum_topic_hidden() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-forum-hidden";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = forum_msg_base(42);
        json["general_forum_topic_hidden"] = serde_json::json!({});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.forum.general_hidden", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_general_forum_topic_hidden(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        recv(&mut sub).await;
    }

    #[tokio::test]
    async fn test_nats_general_forum_topic_unhidden() {
        let Some((client, js)) = try_connect().await else {
            eprintln!("SKIP");
            return;
        };
        let prefix = "integ-forum-unhidden";
        let bridge = make_bridge(client.clone(), js, prefix).await;

        let mut json = forum_msg_base(42);
        json["general_forum_topic_unhidden"] = serde_json::json!({});
        let message: Message = serde_json::from_value(json).unwrap();

        let subject = format!("telegram.{}.bot.forum.general_unhidden", prefix);
        let mut sub = client.subscribe(subject).await.unwrap();
        handlers::handle_general_forum_topic_unhidden(fake_bot(), message, bridge, health())
            .await
            .unwrap();

        recv(&mut sub).await;
    }
}
