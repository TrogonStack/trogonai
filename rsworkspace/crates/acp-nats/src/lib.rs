pub mod agent;
pub mod config;
pub mod error;
pub mod nats;
pub mod prompt_event;
pub mod session_id;
pub(crate) mod telemetry;

pub use agent::Bridge;
pub use config::{AcpPrefix, Config, ValidationError};
pub use error::AGENT_UNAVAILABLE;
pub use nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
pub use prompt_event::{PromptEvent, PromptPayload};
pub use session_id::AcpSessionId;
pub use trogon_nats::{NatsAuth, NatsConfig};
