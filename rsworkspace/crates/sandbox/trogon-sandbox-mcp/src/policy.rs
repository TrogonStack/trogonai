//! Role scoping for one server process: which tools are offered, which commands may run and where the file tools
//! may reach.
//!
//! Tool names are never listed here. Patterns are matched against whatever the router has registered, so a new
//! tool is allowed or denied by the same rules without a code change.

mod command_policy;
mod command_veto_error;
mod confined_path;
mod path_veto_error;
mod probe;
mod root_policy;
mod root_policy_error;
mod tool_filter;
mod verify_no_symlinks;

pub use command_policy::CommandPolicy;
pub use command_veto_error::CommandVetoError;
pub use confined_path::ConfinedPath;
pub use path_veto_error::PathVetoError;
pub use probe::Probe;
pub use root_policy::RootPolicy;
pub use root_policy_error::RootPolicyError;
pub use tool_filter::ToolFilter;
pub use verify_no_symlinks::verify_no_symlinks;

/// Everything one server process enforces on its client.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub tools: ToolFilter,
    pub commands: CommandPolicy,
    pub roots: RootPolicy,
}
