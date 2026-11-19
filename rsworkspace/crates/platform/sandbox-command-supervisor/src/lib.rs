mod arguments;
mod cancellation;
mod certainty;
mod command;
mod command_timeout;
mod command_wire;
mod constants;
mod delivery;
mod entrypoint;
mod environment;
mod execution_id;
#[cfg(target_os = "linux")]
mod linux;
mod platform_support;
pub mod protocol;
mod protocol_error;
mod protocol_version;
mod release_state;
mod release_timeout;
mod signal_number;
mod signal_request_id;
mod timeout_state;
mod truncation;
mod working_directory;

pub fn run() -> i32 {
    entrypoint::run(protocol::MAX_CHILDREN)
}

#[cfg(feature = "test-support")]
pub mod test_support;

pub use arguments::Arguments;
pub use cancellation::Cancellation;
pub use certainty::Certainty;
pub use command::Command;
pub use command_timeout::CommandTimeout;
pub use command_wire::CommandWire;
pub use delivery::Delivery;
pub use environment::Environment;
pub use execution_id::{ExecutionId, ExecutionIdError};
pub use platform_support::PlatformSupport;
pub use protocol_error::ProtocolError;
pub use protocol_version::ProtocolVersion;
pub use release_state::ReleaseState;
pub use release_timeout::ReleaseTimeout;
pub use signal_number::SignalNumber;
pub use signal_request_id::SignalRequestId;
pub use timeout_state::TimeoutState;
pub use truncation::Truncation;
pub use working_directory::WorkingDirectory;
