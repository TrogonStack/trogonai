//! NATS JetStream KV client for idempotent config projection.

mod kv;

pub use kv::{
    ConfigKv, ConfigKvError, ConfigKvPutOutcome, MemoryConfigKv, NatsConfigKv, PutOptions, open_default_config_kv,
};
