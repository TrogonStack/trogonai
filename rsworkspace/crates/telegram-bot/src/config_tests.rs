#[cfg(test)]
mod tests {
    use crate::config::*;

    #[test]
    fn test_default_feature_config() {
        let config = FeatureConfig::default();
        assert!(config.inline_buttons);
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

    // ── StreamingMode serde ───────────────────────────────────────────────────

    #[test]
    fn test_streaming_mode_toml_serde() {
        // TOML requires a key, so we wrap in a helper struct
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Helper {
            streaming: StreamingMode,
        }

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

    // ── inbound_stream_name defaults ─────────────────────────────────────────

    #[test]
    fn test_default_inbound_stream_name() {
        let toml = r#"
[telegram]
bot_token = "test-token"

[nats]
servers = ["localhost:4222"]
prefix = "test"
"#;
        let config: Config = toml::from_str(toml).expect("parse config");
        assert_eq!(config.inbound_stream_name, "TELEGRAM");
    }
}
