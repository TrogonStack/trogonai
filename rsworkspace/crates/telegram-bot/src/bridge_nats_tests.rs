//! Integration tests for TelegramBridge — require a live NATS + JetStream.
//!
//! Run with: NATS_URL=nats://localhost:14222 cargo test bridge_nats
//!
//! Tests are skipped automatically when NATS is unreachable.

#[cfg(test)]
mod nats_tests {
    use crate::bridge::TelegramBridge;
    use async_nats::jetstream;
    use futures::StreamExt;
    use telegram_types::{
        policies::{DmPolicy, GroupPolicy},
        AccessConfig,
    };
    use teloxide::types::Message;

    const DEFAULT_NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<(async_nats::Client, jetstream::Context)> {
        let url =
            std::env::var("NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
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

    async fn recv(sub: &mut async_nats::Subscriber) -> serde_json::Value {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            sub.next(),
        )
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
}
