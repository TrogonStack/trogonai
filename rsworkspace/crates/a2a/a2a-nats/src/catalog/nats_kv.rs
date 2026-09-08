use async_nats::jetstream::kv;

pub use crate::constants::A2A_AGENT_CARDS;
use crate::constants::MAX_VALUE_SIZE;

pub fn catalog_bucket_config() -> kv::Config {
    kv::Config {
        bucket: A2A_AGENT_CARDS.to_owned(),
        history: 1,
        max_value_size: MAX_VALUE_SIZE,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
