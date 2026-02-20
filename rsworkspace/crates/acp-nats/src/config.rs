use trogon_nats::NatsConfig;

pub struct Config {
    pub acp_prefix: String,
    pub nats: NatsConfig,
}

impl Config {
    pub fn new(acp_prefix: String, nats: NatsConfig) -> Self {
        Self { acp_prefix, nats }
    }
}
