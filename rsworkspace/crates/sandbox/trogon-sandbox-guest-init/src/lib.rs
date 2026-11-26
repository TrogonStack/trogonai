//! Parsing of the kernel command line a sandbox guest boots with. The guest init binary reads it
//! to learn the actor it serves and the vsock port its agent listens on.

mod guest_agent_port;
mod guest_agent_port_error;
mod guest_boot_config;

pub use guest_agent_port::GuestAgentPort;
pub use guest_agent_port_error::GuestAgentPortError;
pub use guest_boot_config::GuestBootConfig;
