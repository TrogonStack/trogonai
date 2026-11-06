mod approved_registration;
mod error;
mod producer_config;

pub use approved_registration::ApprovedRegistration;
pub use error::RegistrationError;
pub use producer_config::ProducerConfig;

pub const SIGNING_CONTEXT: &[u8] = b"trogon/workload-registration/v1\0";
