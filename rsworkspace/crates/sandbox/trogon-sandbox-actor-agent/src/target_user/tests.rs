use std::error::Error;
use std::ffi::CString;
use std::sync::Mutex;

use super::*;

fn current() -> Result<nix::unistd::User, Box<dyn Error>> {
    nix::unistd::User::from_uid(Uid::effective())?.ok_or_else(|| "the running uid has no passwd entry".into())
}

#[test]
fn an_unknown_user_is_an_invalid_spec_rather_than_a_backend_fault() -> Result<(), Box<dyn Error>> {
    let username = SandboxUserName::new("definitely-not-a-user-xyz")?;
    let error = TargetUser::resolve(&username).err();
    assert!(matches!(error, Some(LeaseError::InvalidSpec { .. })), "{error:?}");
    Ok(())
}

#[test]
fn the_agents_own_user_resolves_without_needing_root() -> Result<(), Box<dyn Error>> {
    let current = current()?;
    let username = SandboxUserName::new(current.name.clone())?;
    let resolved = TargetUser::resolve(&username)?;
    assert_eq!(resolved.name(), current.name);
    assert_eq!(resolved.home(), current.dir.to_string_lossy());
    Ok(())
}

#[test]
fn a_non_root_agent_cannot_become_another_user() -> Result<(), Box<dyn Error>> {
    let username = SandboxUserName::new("someone")?;
    let refused = require_privilege_to_become(Uid::from_raw(1000), Uid::from_raw(1001), &username);
    assert!(matches!(refused, Err(LeaseError::Backend { .. })));
    assert!(require_privilege_to_become(Uid::from_raw(1000), Uid::from_raw(1000), &username).is_ok());
    assert!(require_privilege_to_become(Uid::from_raw(0), Uid::from_raw(1001), &username).is_ok());
    Ok(())
}

#[test]
fn applying_the_agents_own_user_sets_identity_without_a_drop() -> Result<(), Box<dyn Error>> {
    let current = current()?;
    let user = TargetUser {
        name: current.name,
        uid: current.uid,
        gid: current.gid,
        home: "/home/own".to_string(),
    };
    let mut command = Command::new("true");
    user.apply(&mut command);
    let envs: Vec<_> = command.as_std().get_envs().collect();
    assert!(envs.iter().any(|(key, value)| *key == "HOME" && value.is_some()));
    Ok(())
}

#[test]
fn applying_another_user_registers_the_privilege_drop() {
    let user = TargetUser {
        name: "other".to_string(),
        uid: Uid::from_raw(Uid::effective().as_raw().wrapping_add(1)),
        gid: Gid::from_raw(0),
        home: "/home/other".to_string(),
    };
    let mut command = Command::new("true");
    user.apply(&mut command);
    assert_eq!(command.as_std().get_program(), "true");
}

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<&'static str>>,
    fail_at: Option<&'static str>,
}

impl Recorder {
    fn step(&self, name: &'static str) -> std::io::Result<()> {
        self.calls
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .push(name);
        if self.fail_at == Some(name) {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
}

impl Privileges for Recorder {
    fn init_groups(&self, _name: &std::ffi::CStr, _gid: libc::gid_t) -> std::io::Result<()> {
        self.step("initgroups")
    }

    fn set_gid(&self, _gid: libc::gid_t) -> std::io::Result<()> {
        self.step("setgid")
    }

    fn set_uid(&self, _uid: libc::uid_t) -> std::io::Result<()> {
        self.step("setuid")
    }
}

#[test]
fn the_drop_gives_up_groups_before_the_user_and_stops_at_the_first_failure() -> Result<(), Box<dyn Error>> {
    let name = CString::new("user")?;
    for (fail_at, expected) in [
        (None, vec!["initgroups", "setgid", "setuid"]),
        (Some("initgroups"), vec!["initgroups"]),
        (Some("setgid"), vec!["initgroups", "setgid"]),
        (Some("setuid"), vec!["initgroups", "setgid", "setuid"]),
    ] {
        let recorder = Recorder {
            fail_at,
            ..Recorder::default()
        };
        let result = drop_privileges(&recorder, &name, 1, 1);
        assert_eq!(result.is_ok(), fail_at.is_none());
        let calls = recorder.calls.lock().map_err(|error| error.to_string())?;
        assert_eq!(*calls, expected);
    }
    Ok(())
}

#[test]
fn the_libc_calls_accept_the_agents_own_identity_or_report_missing_privilege() -> Result<(), Box<dyn Error>> {
    let name = CString::new("user")?;
    let (uid, gid) = (Uid::effective().as_raw(), Gid::effective().as_raw());
    assert!(LibcPrivileges.set_gid(gid).is_ok());
    assert!(LibcPrivileges.set_uid(uid).is_ok());
    assert_eq!(
        LibcPrivileges.init_groups(&name, gid).is_ok(),
        Uid::effective().is_root()
    );
    Ok(())
}
