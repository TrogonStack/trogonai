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
mod tests;
