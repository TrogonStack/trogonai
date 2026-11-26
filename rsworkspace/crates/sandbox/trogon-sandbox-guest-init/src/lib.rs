//! Parsing of the kernel command line a sandbox guest boots with. The guest init binary reads it
//! to learn the actor it serves and the vsock port its agent listens on.

mod guest_actor;
mod guest_actor_id;
mod guest_actor_id_error;
mod guest_agent_port;
mod guest_agent_port_error;
mod guest_boot_config;
mod guest_boot_config_error;

pub use guest_actor::GuestActor;
pub use guest_actor_id::GuestActorId;
pub use guest_actor_id_error::GuestActorIdError;
pub use guest_agent_port::GuestAgentPort;
pub use guest_agent_port_error::GuestAgentPortError;
pub use guest_boot_config::GuestBootConfig;
pub use guest_boot_config_error::GuestBootConfigError;
