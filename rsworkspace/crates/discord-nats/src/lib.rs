//! NATS messaging infrastructure for Discord integration

pub mod clock;
pub mod config;
pub mod error;
pub mod messaging;
pub mod mock;
pub mod nats;
pub mod subjects;

pub use clock::{Clock, MockClock, SystemClock};
pub use config::NatsConfig;
pub use error::{Error, Result};
pub use messaging::{MessagePublisher, MessageSubscriber, Publish};
pub use mock::MockPublisher;
pub use nats::connect;
