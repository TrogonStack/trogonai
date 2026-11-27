use std::ffi::CString;

use nix::unistd::{Gid, Uid};
use tokio::process::Command;
use trogon_sandbox_contract::{LeaseError, SandboxUserName};

use crate::lease_fault::{backend, invalid_spec};
use crate::syscall_result::syscall_result;

/// A sandbox user resolved against the actor's own passwd database.
///
/// Carrying the resolved ids rather than the name is what lets exec and file operations drop privileges before the
/// process ever runs, instead of asking a shell to do it afterwards.
#[derive(Debug, Clone)]
pub struct TargetUser {
    name: String,
    uid: Uid,
    gid: Gid,
    home: String,
}

impl TargetUser {
    pub fn home(&self) -> &str {
        &self.home
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Looks a sandbox user up and refuses now rather than at spawn time.
    ///
    /// Both refusals here would otherwise surface as an opaque failure to start the command: a user the tenant never
    /// created, and an agent that is not root and so cannot become anyone else.
    pub fn resolve(username: &SandboxUserName) -> Result<Self, LeaseError> {
        let missing = || {
            invalid_spec(format!(
                "sandbox user '{}' does not exist in this sandbox",
                username.as_str()
            ))
        };
        let found = nix::unistd::User::from_name(username.as_str());
        let user = found.map_err(|error| backend(format!("failed to read the passwd database: {error}")))?;
        let user = user.ok_or_else(missing)?;
        require_privilege_to_become(Uid::effective(), user.uid, username)?;
        Ok(Self {
            name: user.name,
            uid: user.uid,
            gid: user.gid,
            home: user.dir.to_string_lossy().into_owned(),
        })
    }

    /// Makes `command` run as this user.
    ///
    /// The drop happens in `pre_exec` rather than through `Command::uid`, because supplementary groups need
    /// `initgroups` and there is no stable `Command` method for them; without it a user added to a group would still
    /// be refused by that group's shared directory, which is the only reason the group exists.
    pub fn apply(&self, command: &mut Command) {
        command.env("HOME", &self.home);
        command.env("USER", &self.name);
        command.env("LOGNAME", &self.name);

        // Already this user: there is nothing to drop, and `initgroups` would need the root the agent does not have.
        if Uid::effective() == self.uid {
            return;
        }

        let name = CString::new(self.name.as_str()).unwrap_or_default();
        let (uid, gid) = (self.uid.as_raw(), self.gid.as_raw());
        // SAFETY: the closure only calls async-signal-safe functions between fork and exec.
        unsafe { command.pre_exec(move || drop_privileges(&LibcPrivileges, &name, uid, gid)) };
    }
}

fn require_privilege_to_become(effective: Uid, target: Uid, username: &SandboxUserName) -> Result<(), LeaseError> {
    if !effective.is_root() && effective != target {
        return Err(backend(format!(
            "the agent is not running as root inside the actor, so it cannot run as '{}'",
            username.as_str()
        )));
    }
    Ok(())
}

trait Privileges {
    fn init_groups(&self, name: &std::ffi::CStr, gid: libc::gid_t) -> std::io::Result<()>;
    fn set_gid(&self, gid: libc::gid_t) -> std::io::Result<()>;
    fn set_uid(&self, uid: libc::uid_t) -> std::io::Result<()>;
}

/// Order matters: both `initgroups` and `setgid` need privileges that `setuid` gives away, so `setuid` is last.
fn drop_privileges(
    privileges: &impl Privileges,
    name: &std::ffi::CStr,
    uid: libc::uid_t,
    gid: libc::gid_t,
) -> std::io::Result<()> {
    privileges.init_groups(name, gid)?;
    privileges.set_gid(gid)?;
    privileges.set_uid(uid)
}

/// `libc` rather than `nix` because `initgroups` is not exposed for every target nix supports, and losing
/// supplementary groups silently is worse than the raw call.
struct LibcPrivileges;

impl Privileges for LibcPrivileges {
    fn init_groups(&self, name: &std::ffi::CStr, gid: libc::gid_t) -> std::io::Result<()> {
        // SAFETY: `name` is a valid NUL-terminated string for the duration of the call.
        syscall_result(unsafe { libc::initgroups(name.as_ptr(), gid as _) })
    }

    fn set_gid(&self, gid: libc::gid_t) -> std::io::Result<()> {
        // SAFETY: plain syscall wrapper with no pointer arguments.
        syscall_result(unsafe { libc::setgid(gid) })
    }

    fn set_uid(&self, uid: libc::uid_t) -> std::io::Result<()> {
        // SAFETY: plain syscall wrapper with no pointer arguments.
        syscall_result(unsafe { libc::setuid(uid) })
    }
}

#[cfg(test)]
mod tests;
