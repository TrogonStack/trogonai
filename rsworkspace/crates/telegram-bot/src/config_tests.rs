#[cfg(test)]
mod tests {
    use crate::config::*;

    #[test]
    fn test_default_feature_config() {
        let config = FeatureConfig::default();
        assert_eq!(config.inline_buttons, true);
        assert_eq!(config.streaming, StreamingMode::Partial);
    }

    #[test]
    fn test_default_limit_config() {
        let config = LimitConfig::default();
        assert_eq!(config.text_chunk_limit, 4096);
        assert_eq!(config.media_max_mb, 50);
        assert_eq!(config.history_limit, 100);
        assert_eq!(config.rate_limit_messages_per_minute, 20);
    }

    #[test]
    fn test_streaming_mode_serialization() {
        let partial = StreamingMode::Partial;
        let disabled = StreamingMode::Disabled;
        let full = StreamingMode::Full;

        // Just verify they can be created
        assert_eq!(partial, StreamingMode::Partial);
        assert_eq!(disabled, StreamingMode::Disabled);
        assert_eq!(full, StreamingMode::Full);
    }

    // ── UpdateModeConfig defaults ──────────────────────────────────────────────

    #[test]
    fn test_default_update_mode_is_polling() {
        match UpdateModeConfig::default() {
            UpdateModeConfig::Polling { timeout, limit } => {
                assert_eq!(timeout, 30);
                assert_eq!(limit, 100);
            }
            other => panic!("Expected Polling, got {:?}", other),
        }
    }

    // ── TOML parsing: polling ──────────────────────────────────────────────────

    #[test]
    fn test_toml_polling_defaults() {
        let toml = r#"
[telegram]
bot_token = "test-token"

[nats]
servers = ["localhost:4222"]
prefix = "test"
"#;
        let config: Config = toml::from_str(toml).expect("parse config");
        match config.telegram.update_mode {
            UpdateModeConfig::Polling { timeout, limit } => {
                assert_eq!(timeout, 30);
                assert_eq!(limit, 100);
            }
            other => panic!("Expected Polling, got {:?}", other),
        }
    }

    #[test]
    fn test_toml_polling_custom_values() {
        let toml = r#"
[telegram]
bot_token = "test-token"

[telegram.update_mode]
mode = "polling"
timeout = 60
limit = 50

[nats]
servers = ["localhost:4222"]
prefix = "test"
"#;
        let config: Config = toml::from_str(toml).expect("parse config");
        match config.telegram.update_mode {
            UpdateModeConfig::Polling { timeout, limit } => {
                assert_eq!(timeout, 60);
                assert_eq!(limit, 50);
            }
            other => panic!("Expected Polling, got {:?}", other),
        }
    }

    // ── TOML parsing: webhook ──────────────────────────────────────────────────

    #[test]
    fn test_toml_webhook_required_fields() {
        let toml = r#"
[telegram]
bot_token = "test-token"

[telegram.update_mode]
mode = "webhook"
url = "https://example.com/webhook"

[nats]
servers = ["localhost:4222"]
prefix = "test"
"#;
        let config: Config = toml::from_str(toml).expect("parse config");
        match config.telegram.update_mode {
            UpdateModeConfig::Webhook {
                url,
                port,
                path,
                secret_token,
                bind_address,
                max_connections,
            } => {
                assert_eq!(url, "https://example.com/webhook");
                assert_eq!(port, 8443);
                assert_eq!(path, "/webhook");
                assert!(secret_token.is_none());
                assert_eq!(bind_address, "0.0.0.0");
                assert_eq!(max_connections, 40);
            }
            other => panic!("Expected Webhook, got {:?}", other),
        }
    }

    #[test]
    fn test_toml_webhook_all_fields() {
        let toml = r#"
[telegram]
bot_token = "test-token"

[telegram.update_mode]
mode = "webhook"
url = "https://bot.example.com/tg"
port = 443
path = "/tg"
secret_token = "mysecret123"
bind_address = "127.0.0.1"
max_connections = 10

[nats]
servers = ["localhost:4222"]
prefix = "test"
"#;
        let config: Config = toml::from_str(toml).expect("parse config");
        match config.telegram.update_mode {
            UpdateModeConfig::Webhook {
                url,
                port,
                path,
                secret_token,
                bind_address,
                max_connections,
            } => {
                assert_eq!(url, "https://bot.example.com/tg");
                assert_eq!(port, 443);
                assert_eq!(path, "/tg");
                assert_eq!(secret_token, Some("mysecret123".to_string()));
                assert_eq!(bind_address, "127.0.0.1");
                assert_eq!(max_connections, 10);
            }
            other => panic!("Expected Webhook, got {:?}", other),
        }
    }

    // ── UpdateModeConfig TOML serde roundtrip ─────────────────────────────────

    #[test]
    fn test_webhook_config_serde_roundtrip() {
        let original = UpdateModeConfig::Webhook {
            url: "https://example.com/webhook".to_string(),
            port: 8443,
            path: "/webhook".to_string(),
            secret_token: Some("token123".to_string()),
            bind_address: "0.0.0.0".to_string(),
            max_connections: 40,
        };

        let toml_str = toml::to_string(&original).expect("serialize");
        let decoded: UpdateModeConfig = toml::from_str(&toml_str).expect("deserialize");

        match decoded {
            UpdateModeConfig::Webhook {
                url,
                port,
                path,
                secret_token,
                bind_address,
                max_connections,
            } => {
                assert_eq!(url, "https://example.com/webhook");
                assert_eq!(port, 8443);
                assert_eq!(path, "/webhook");
                assert_eq!(secret_token, Some("token123".to_string()));
                assert_eq!(bind_address, "0.0.0.0");
                assert_eq!(max_connections, 40);
            }
            other => panic!("Expected Webhook, got {:?}", other),
        }
    }

    #[test]
    fn test_polling_config_serde_roundtrip() {
        let original = UpdateModeConfig::Polling {
            timeout: 45,
            limit: 75,
        };

        let toml_str = toml::to_string(&original).expect("serialize");
        let decoded: UpdateModeConfig = toml::from_str(&toml_str).expect("deserialize");

        match decoded {
            UpdateModeConfig::Polling { timeout, limit } => {
                assert_eq!(timeout, 45);
                assert_eq!(limit, 75);
            }
            other => panic!("Expected Polling, got {:?}", other),
        }
    }

    // ── StreamingMode serde ───────────────────────────────────────────────────

    #[test]
    fn test_streaming_mode_toml_serde() {
        // TOML requires a key, so we wrap in a helper struct
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Helper { streaming: StreamingMode }

        for (variant, expected) in [
            ("full", StreamingMode::Full),
            ("disabled", StreamingMode::Disabled),
            ("partial", StreamingMode::Partial),
        ] {
            let toml_str = format!(r#"streaming = "{}""#, variant);
            let h: Helper = toml::from_str(&toml_str).expect("deserialize");
            assert_eq!(h.streaming, expected);
        }
    }
}
