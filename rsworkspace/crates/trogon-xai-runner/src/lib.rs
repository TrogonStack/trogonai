mod agent;
pub mod agent_loader;
mod client;
mod http_client;
mod session_notifier;
pub mod session_store;
pub mod skill_loader;

pub use agent::XaiAgent;
pub use agent_loader::{AgentConfig, AgentLoader, AgentLoading};
pub use client::{FinishReason, InputItem, Message, XaiClient, XaiEvent};
pub use http_client::XaiHttpClient;
pub use session_notifier::{NatsSessionNotifier, SessionNotifier};
pub use session_store::{NatsSessionStore, SessionStoring};
pub use skill_loader::{SkillLoader, SkillLoading};

#[cfg(feature = "test-helpers")]
pub use http_client::mock::{MockCall, MockResponse, MockXaiHttpClient};
#[cfg(feature = "test-helpers")]
pub use session_notifier::MockSessionNotifier;
