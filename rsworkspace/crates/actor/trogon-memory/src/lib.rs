#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(coverage, allow(dead_code))] // coverage build cfg-excludes `serve`, orphaning its private server helpers
// Trait impls intentionally return `impl Future + Send` to pin the auto-trait
// bound; converting to `async fn` would drop the explicit `Send` guarantee.
#![allow(clippy::manual_async_fn)]
pub mod config;
pub mod dreamer;
pub mod provider;
pub mod provision;
pub mod server;
pub mod service;
pub mod store;
pub mod types;
pub mod writer;

pub use config::DreamingConfig;
pub use dreamer::Dreamer;
pub use provider::{AnthropicMemoryProvider, MemoryAuthStyle, MemoryLlmConfig, MemoryProvider};
pub use provision::{DREAMS_STREAM, MEMORIES_BUCKET, provision_kv, provision_stream};
pub use service::{DreamingService, trigger_dreaming};
pub use store::{MemoryClient, MemoryStore, memory_key};
pub use types::{DreamTrigger, DreamerError, EntityMemory, MemoryFact, RawFact};
pub use writer::{MemoryWriteHandler, MemoryWriter, WriteRequest, WriteResponse, write_memory};

pub use server::serve;
