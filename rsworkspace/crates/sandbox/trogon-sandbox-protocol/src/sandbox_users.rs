use trogon_sandbox_contract::{SandboxGroupName, SandboxUserName};

/// The home directory a sandbox user gets, `/home/<user>`.
///
/// Every provider that implements sandbox users agrees on this path, because a tenant that creates a user on one
/// provider and reads the home directory back uses the path in a command.
pub fn home_dir_for_user(username: &SandboxUserName) -> String {
    format!("/home/{}", username.as_str())
}

/// The shared directory a sandbox group's members read and write through, `/shared/<group>`.
///
/// It is created setgid so a file written by any member inherits the group rather than the writer's own primary group.
pub fn shared_dir_for_group(groupname: &SandboxGroupName) -> String {
    format!("/shared/{}", groupname.as_str())
}
