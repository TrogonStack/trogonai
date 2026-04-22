mod agent;
mod agent_loader;
mod client;
mod console_session;
mod http_client;
mod session_notifier;
mod session_store;
mod skill_loader;

pub use agent::XaiAgent;
pub use client::{FinishReason, InputItem, Message, XaiClient, XaiEvent};
pub use http_client::XaiHttpClient;
pub use session_notifier::{NatsSessionNotifier, SessionNotifier};

#[cfg(feature = "test-helpers")]
pub use http_client::mock::{MockCall, MockResponse, MockXaiHttpClient};
#[cfg(feature = "test-helpers")]
pub use session_notifier::MockSessionNotifier;
#[cfg(feature = "test-helpers")]
pub use session_store::{MemorySessionStore, SessionStore, XaiSessionData};
