//! NATS JetStream KV client for idempotent config projection.

mod kv;

pub use kv::{
    open_default_config_kv, ConfigKv, ConfigKvError, ConfigKvPutOutcome, MemoryConfigKv,
    NatsConfigKv, PutOptions,
};
