//! NATS messaging infrastructure for Discord integration

pub mod clock;
pub mod config;
pub mod error;
pub mod messaging;
pub mod nats;
pub mod subjects;

pub use clock::{Clock, MockClock, SystemClock};
pub use config::NatsConfig;
pub use error::{Error, Result};
pub use messaging::{MessagePublisher, MessageSubscriber, Publish, QueueSubscribeClient};
pub use nats::connect;

#[cfg(feature = "test-support")]
pub mod mock;
#[cfg(feature = "test-support")]
pub use mock::MockPublisher;
#[cfg(feature = "test-support")]
pub use trogon_nats::MockNatsClient;
