use super::*;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore, StreamMaxAge,
    };
    use trogon_std::NonZeroDuration;

    fn wrap_publisher(
        publisher: MockJetStreamPublisher,
    ) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
        ClaimCheckPublisher::new(
            publisher,
            MockObjectStore::new(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(usize::MAX),
        )
    }

    fn bridge() -> GatewayBridge<MockJetStreamPublisher, MockObjectStore> {
        GatewayBridge::new(
            wrap_publisher(MockJetStreamPublisher::new()),
            NatsToken::new("discord").unwrap(),
            Duration::from_secs(5),
        )
    }

    fn discord_config() -> DiscordConfig {
        DiscordConfig {
            bot_token: super::super::config::DiscordBotToken::new("Bot token").unwrap(),
            intents: twilight_model::gateway::Intents::GUILDS,
            subject_prefix: NatsToken::new("discord").unwrap(),
            stream_name: NatsToken::new("DISCORD").unwrap(),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
        }
    }

    #[tokio::test]
    async fn provision_creates_stream() {
        let js = MockJetStreamContext::new();
        let config = discord_config();

        provision(&js, &config).await.unwrap();

        let streams = js.created_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "DISCORD");
        assert_eq!(streams[0].subjects, vec!["discord.>"]);
        assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn provision_propagates_error() {
        let js = MockJetStreamContext::new();
        js.fail_next();
        let config = discord_config();

        let result = provision(&js, &config).await;
        assert!(result.is_err());
    }

    fn bridge_with_mock() -> (
        GatewayBridge<MockJetStreamPublisher, MockObjectStore>,
        MockJetStreamPublisher,
    ) {
        let mock = MockJetStreamPublisher::new();
        let b = GatewayBridge::new(
            wrap_publisher(mock.clone()),
            NatsToken::new("discord").unwrap(),
            Duration::from_secs(5),
        );
        (b, mock)
    }

    fn bridge_with_prefix(
        prefix: &str,
    ) -> (
        GatewayBridge<MockJetStreamPublisher, MockObjectStore>,
        MockJetStreamPublisher,
    ) {
        let mock = MockJetStreamPublisher::new();
        let b = GatewayBridge::new(
            wrap_publisher(mock.clone()),
            NatsToken::new(prefix).unwrap(),
            Duration::from_secs(5),
        );
        (b, mock)
    }

    #[tokio::test]
    async fn publish_sends_to_correct_subject() {
        let (b, mock) = bridge_with_mock();
        b.publish("message_create", None, None, &serde_json::json!({"test": true}))
            .await;

        let subjects = mock.published_subjects();
        assert_eq!(subjects, vec!["discord.message_create"]);
    }

    #[tokio::test]
    async fn publish_uses_configured_prefix() {
        let (b, mock) = bridge_with_prefix("dc");
        b.publish("ready", None, None, &serde_json::json!({})).await;

        let subjects = mock.published_subjects();
        assert_eq!(subjects, vec!["dc.ready"]);
    }

    #[tokio::test]
    async fn publish_includes_event_name_header() {
        let (b, mock) = bridge_with_mock();
        b.publish("guild_create", Some(123), None, &serde_json::json!({})).await;

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        let headers = &msgs[0].headers;
        assert_eq!(
            headers.get(NATS_HEADER_EVENT_NAME).map(|v| v.as_str()),
            Some("guild_create")
        );
    }

    #[tokio::test]
    async fn publish_includes_guild_id_header_when_present() {
        let (b, mock) = bridge_with_mock();
        b.publish("message_create", Some(999), None, &serde_json::json!({}))
            .await;

        let msgs = mock.published_messages();
        let headers = &msgs[0].headers;
        assert_eq!(headers.get(NATS_HEADER_GUILD_ID).map(|v| v.as_str()), Some("999"));
    }

    #[tokio::test]
    async fn publish_omits_guild_id_header_when_absent() {
        let (b, mock) = bridge_with_mock();
        b.publish("user_update", None, None, &serde_json::json!({})).await;

        let msgs = mock.published_messages();
        let headers = &msgs[0].headers;
        assert!(headers.get(NATS_HEADER_GUILD_ID).is_none());
    }

    #[tokio::test]
    async fn publish_serializes_payload_as_json() {
        let (b, mock) = bridge_with_mock();

        #[derive(serde::Serialize)]
        struct TestPayload {
            channel_id: u64,
            content: String,
        }

        b.publish(
            "message_create",
            None,
            None,
            &TestPayload {
                channel_id: 42,
                content: "hello".to_string(),
            },
        )
        .await;

        let payloads = mock.published_payloads();
        let parsed: serde_json::Value = serde_json::from_slice(&payloads[0]).expect("valid json");
        assert_eq!(parsed["channel_id"], 42);
        assert_eq!(parsed["content"], "hello");
    }

    #[tokio::test]
    async fn publish_failure_does_not_panic() {
        let (b, mock) = bridge_with_mock();
        mock.fail_next_js_publish();
        b.publish("message_create", None, None, &serde_json::json!({})).await;

        assert!(mock.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn multiple_publishes_accumulate() {
        let (b, mock) = bridge_with_mock();
        b.publish("message_create", Some(1), None, &serde_json::json!({})).await;
        b.publish("reaction_add", Some(2), None, &serde_json::json!({})).await;
        b.publish("guild_delete", Some(3), None, &serde_json::json!({})).await;

        let subjects = mock.published_subjects();
        assert_eq!(
            subjects,
            vec!["discord.message_create", "discord.reaction_add", "discord.guild_delete",]
        );
    }

    #[tokio::test]
    async fn publish_sets_nats_msg_id_when_provided() {
        let (b, mock) = bridge_with_mock();
        let mid = Some("message_create:123456".to_string());
        b.publish("message_create", Some(1), mid, &serde_json::json!({})).await;

        let msgs = mock.published_messages();
        let headers = &msgs[0].headers;
        assert_eq!(
            headers.get(async_nats::header::NATS_MESSAGE_ID).map(|v| v.as_str()),
            Some("message_create:123456")
        );
    }

    #[tokio::test]
    async fn publish_omits_nats_msg_id_when_none() {
        let (b, mock) = bridge_with_mock();
        b.publish("presence_update", Some(1), None, &serde_json::json!({}))
            .await;

        let msgs = mock.published_messages();
        let headers = &msgs[0].headers;
        assert!(headers.get(async_nats::header::NATS_MESSAGE_ID).is_none());
    }

    #[tokio::test]
    async fn new_sets_fields() {
        let b = bridge();
        assert_eq!(b.subject_prefix.as_str(), "discord");
        assert_eq!(b.nats_ack_timeout, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn dispatch_forwards_dispatch_event() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"123","channel_id":"456","content":"hi"}}"#)
            .await;
        assert_eq!(mock.published_subjects(), vec!["discord.message_create"]);
    }

    #[tokio::test]
    async fn dispatch_forwards_raw_data_payload() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"123","content":"hello world"}}"#)
            .await;

        let payloads = mock.published_payloads();
        let parsed: serde_json::Value = serde_json::from_slice(&payloads[0]).expect("valid json");
        assert_eq!(parsed["id"], "123");
        assert_eq!(parsed["content"], "hello world");
    }

    #[tokio::test]
    async fn dispatch_skips_non_dispatch_ops() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":11}"#).await;
        b.dispatch(r#"{"op":10,"d":{"heartbeat_interval":41250}}"#).await;
        b.dispatch(r#"{"op":7}"#).await;
        b.dispatch(r#"{"op":9,"d":false}"#).await;
        b.dispatch(r#"{"op":1,"d":42}"#).await;
        assert!(mock.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn dispatch_skips_missing_event_type() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"s":1,"d":{}}"#).await;
        assert!(mock.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn dispatch_skips_missing_data() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1}"#).await;
        assert!(mock.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn dispatch_handles_invalid_json() {
        let (b, mock) = bridge_with_mock();
        b.dispatch("not json at all").await;
        assert!(mock.published_subjects().is_empty());
    }

    #[tokio::test]
    async fn dispatch_lowercases_event_type() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"GUILD_MEMBER_ADD","s":1,"d":{"guild_id":"123","user":{"id":"1"}}}"#)
            .await;
        assert_eq!(mock.published_subjects(), vec!["discord.guild_member_add"]);
    }

    #[tokio::test]
    async fn dispatch_includes_guild_id_header() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"1","guild_id":"999"}}"#)
            .await;

        let msgs = mock.published_messages();
        assert_eq!(
            msgs[0].headers.get(NATS_HEADER_GUILD_ID).map(|v| v.as_str()),
            Some("999")
        );
    }

    #[tokio::test]
    async fn dispatch_omits_guild_id_when_absent() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"1"}}"#)
            .await;

        let msgs = mock.published_messages();
        assert!(msgs[0].headers.get(NATS_HEADER_GUILD_ID).is_none());
    }

    #[tokio::test]
    async fn dispatch_sets_dedup_id_for_message_create() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"msg123","channel_id":"456"}}"#)
            .await;

        let msgs = mock.published_messages();
        assert_eq!(
            msgs[0]
                .headers
                .get(async_nats::header::NATS_MESSAGE_ID)
                .map(|v| v.as_str()),
            Some("message_create:msg123")
        );
    }

    #[tokio::test]
    async fn dispatch_no_dedup_for_member_events() {
        let (b, mock) = bridge_with_mock();
        b.dispatch(r#"{"op":0,"t":"GUILD_MEMBER_ADD","s":1,"d":{"guild_id":"1","user":{"id":"2"}}}"#)
            .await;

        let msgs = mock.published_messages();
        assert!(msgs[0].headers.get(async_nats::header::NATS_MESSAGE_ID).is_none());
    }

    #[tokio::test]
    async fn dispatch_publish_failure_does_not_panic() {
        let (b, mock) = bridge_with_mock();
        mock.fail_next_js_publish();
        b.dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"1"}}"#)
            .await;
        assert!(mock.published_subjects().is_empty());
    }

    #[test]
    fn extract_guild_id_from_string() {
        assert_eq!(extract_guild_id(br#"{"guild_id":"123456"}"#), Some(123456));
    }

    #[test]
    fn extract_guild_id_from_number() {
        assert_eq!(extract_guild_id(br#"{"guild_id":789}"#), Some(789));
    }

    #[test]
    fn extract_guild_id_missing() {
        assert_eq!(extract_guild_id(br#"{"id":"1"}"#), None);
    }

    #[test]
    fn extract_guild_id_null() {
        assert_eq!(extract_guild_id(br#"{"guild_id":null}"#), None);
    }

    #[test]
    fn dedup_id_for_message_create() {
        assert_eq!(
            extract_dedup_id("message_create", br#"{"id":"msg1"}"#),
            Some("message_create:msg1".to_string())
        );
    }

    #[test]
    fn dedup_id_for_message_delete() {
        assert_eq!(
            extract_dedup_id("message_delete", br#"{"id":"msg1"}"#),
            Some("message_delete:msg1".to_string())
        );
    }

    #[test]
    fn dedup_id_for_channel_create() {
        assert_eq!(
            extract_dedup_id("channel_create", br#"{"id":"ch1"}"#),
            Some("channel_create:ch1".to_string())
        );
    }

    #[test]
    fn dedup_id_for_message_delete_bulk() {
        assert_eq!(
            extract_dedup_id("message_delete_bulk", br#"{"channel_id":"ch1","ids":["1","2","3"]}"#),
            Some("message_delete_bulk:ch1:1,2,3".to_string())
        );
    }

    #[test]
    fn dedup_id_for_role_create() {
        assert_eq!(
            extract_dedup_id("guild_role_create", br#"{"role":{"id":"r1"}}"#),
            Some("guild_role_create:r1".to_string())
        );
    }

    #[test]
    fn dedup_id_for_role_delete() {
        assert_eq!(
            extract_dedup_id("guild_role_delete", br#"{"role_id":"r1"}"#),
            Some("guild_role_delete:r1".to_string())
        );
    }

    #[test]
    fn dedup_id_for_invite_create() {
        assert_eq!(
            extract_dedup_id("invite_create", br#"{"code":"abc123"}"#),
            Some("invite_create:abc123".to_string())
        );
    }

    #[test]
    fn dedup_id_for_invite_delete() {
        assert_eq!(
            extract_dedup_id("invite_delete", br#"{"code":"xyz"}"#),
            Some("invite_delete:xyz".to_string())
        );
    }

    #[test]
    fn dedup_id_unknown_event_returns_none() {
        assert_eq!(extract_dedup_id("presence_update", br#"{"user":{"id":"1"}}"#), None);
    }

    #[test]
    fn dedup_id_missing_id_field_returns_none() {
        assert_eq!(extract_dedup_id("message_create", br#"{"content":"hi"}"#), None);
    }
