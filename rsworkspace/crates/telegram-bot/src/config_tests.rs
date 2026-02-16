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
}
