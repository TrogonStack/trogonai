//! incident.io webhook receiver that publishes verified events to NATS JetStream.

pub mod config;
pub mod constants;
pub mod incidentio_event_type;
pub mod incidentio_signing_secret;
pub mod server;
pub mod signature;

pub use config::IncidentioConfig;
pub use incidentio_event_type::{IncidentioEventType, IncidentioEventTypeError};
pub use incidentio_signing_secret::{IncidentioSigningSecret, IncidentioSigningSecretError};
pub use server::{provision, router};
pub use signature::SignatureError;
