use async_nats::jetstream::kv;

pub const A2A_AGENT_CARDS: &str = "A2A_AGENT_CARDS";

const MAX_VALUE_SIZE: i32 = 65536;

pub fn catalog_bucket_config() -> kv::Config {
    kv::Config {
        bucket: A2A_AGENT_CARDS.to_owned(),
        history: 1,
        max_value_size: MAX_VALUE_SIZE,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_name_constant() {
        assert_eq!(A2A_AGENT_CARDS, "A2A_AGENT_CARDS");
    }

    #[test]
    fn config_has_history_one() {
        let cfg = catalog_bucket_config();
        assert_eq!(cfg.history, 1);
    }

    #[test]
    fn config_max_value_size_covers_typical_card() {
        let cfg = catalog_bucket_config();
        assert!(cfg.max_value_size >= 65536);
    }
}
