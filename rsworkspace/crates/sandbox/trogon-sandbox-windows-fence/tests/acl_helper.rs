#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use trogon_sandbox_windows_fence::{AclReleaseError, FenceHelper, FsAccessSet, WindowsFence, WindowsSandboxErrorCode};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A stand-in fence helper: a shell script that records its argv and stdin and
/// runs what the test hands it.
struct Fake {
    dir: tempfile::TempDir,
    fence: WindowsFence,
}

impl Fake {
    fn new(script: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let exe = dir.path().join("fence-helper");
        fs::write(
            &exe,
            format!(
                "#!/bin/sh\n[ \"$1\" = __probe ] && exit 0\nprintf '%s\\n' \"$@\" > \"{d}/argv\"\ncat > \"{d}/stdin\"\nshift\n{script}\n",
                d = dir.path().display()
            ),
        )?;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))?;
        wait_until_executable(&exe)?;
        let fence = WindowsFence::new(FenceHelper::new(&exe)?);
        Ok(Self { dir, fence })
    }

    fn argv(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(fs::read_to_string(self.dir.path().join("argv"))?
            .lines()
            .map(String::from)
            .collect())
    }

    fn stdin(&self) -> Result<String, Box<dyn std::error::Error>> {
        Ok(fs::read_to_string(self.dir.path().join("stdin"))?)
    }
}

// A child another test forks while the script is still open for writing keeps that
// descriptor until it execs, and exec of the script fails with ETXTBSY until then.
fn wait_until_executable(exe: &Path) -> TestResult {
    for _ in 0..200 {
        match std::process::Command::new(exe).arg("__probe").status() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(e.into()),
            Ok(_) => return Ok(()),
        }
    }
    Err("the helper stayed busy".into())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn a_persistent_grant_sends_the_access_set_on_stdin() -> TestResult {
    let fake = Fake::new("exit 0")?;
    let access = FsAccessSet {
        grant_write: strings(&["C:/w"]),
        ..FsAccessSet::default()
    };
    fake.fence
        .acl_grant_persistent("S-1", 9, &access, &strings(&["C:/p"]))?;
    assert_eq!(
        fake.argv()?,
        [
            "--fence-helper",
            "acl",
            "grant",
            "--holder-pid",
            "9",
            "--sandbox-user-sid",
            "S-1",
            "--persistent",
            "--allow-profile-exclusion",
            "C:/p"
        ]
    );
    let sent: serde_json::Value = serde_json::from_str(&fake.stdin()?)?;
    assert_eq!(sent["write"], serde_json::json!(["C:/w"]));
    Ok(())
}

#[test]
fn a_session_grant_sends_read_and_write_roots() -> TestResult {
    let fake = Fake::new("exit 0")?;
    fake.fence
        .acl_grant("S-1", 3, &strings(&["C:/r"]), &strings(&["C:/w"]), &[])?;
    let sent: serde_json::Value = serde_json::from_str(&fake.stdin()?)?;
    assert_eq!(
        sent,
        serde_json::json!({"read":["C:/r"],"write":["C:/w"],"denyRead":[],"denyWrite":[]})
    );
    assert!(!fake.argv()?.contains(&"--persistent".to_string()));
    Ok(())
}

#[test]
fn a_refused_grant_carries_the_grant_code_and_the_exit() -> TestResult {
    let fake = Fake::new("echo nope >&2; exit 1")?;
    let Err(error) = fake.fence.acl_grant("S-1", 3, &[], &[], &[]) else {
        return Err("expected an error".into());
    };
    assert_eq!(error.code(), WindowsSandboxErrorCode::AclGrantFailed);
    assert!(error.message().contains("exited 1 (failed): nope"));
    Ok(())
}

#[test]
fn a_stamp_sends_the_denies_and_reports_a_partial_refusal() -> TestResult {
    let fake = Fake::new("exit 0")?;
    fake.fence
        .acl_stamp("S-1", 4, &strings(&["C:/a"]), &strings(&["C:/b"]))?;
    let sent: serde_json::Value = serde_json::from_str(&fake.stdin()?)?;
    assert_eq!(sent["denyRead"], serde_json::json!(["C:/a"]));
    assert_eq!(sent["denyWrite"], serde_json::json!(["C:/b"]));
    assert_eq!(&fake.argv()?[1..3], ["acl", "stamp"]);

    let partial = Fake::new("echo skipped >&2; exit 2")?;
    let Err(error) = partial.fence.acl_stamp("S-1", 4, &[], &[]) else {
        return Err("expected an error".into());
    };
    assert_eq!(error.code(), WindowsSandboxErrorCode::AclStampFailed);
    assert!(error.message().contains("(partial - some inputs skipped): skipped"));
    Ok(())
}

#[test]
fn a_complete_revoke_lists_its_outcomes() -> TestResult {
    let fake = Fake::new(r#"echo '[{"path":"a","status":"revoked"}]'"#)?;
    let revoked = fake.fence.acl_revoke("S-1", 5);
    assert!(revoked.complete);
    assert_eq!(revoked.outcomes.map(|outcomes| outcomes.len()), Some(1));
    assert!(revoked.pending.is_empty());
    assert_eq!(&fake.argv()?[1..3], ["acl", "revoke"]);
    assert_eq!(fake.argv()?.last().map(String::as_str), Some("--json"));
    Ok(())
}

#[test]
fn a_revoke_whose_helper_cannot_run_is_incomplete_with_the_reason() -> TestResult {
    let fake = Fake::new("exit 0")?;
    fs::remove_file(fake.fence.fence_helper().exe())?;
    let revoked = fake.fence.acl_revoke("S-1", 5);
    assert!(!revoked.complete);
    assert_eq!(revoked.outcomes, None);
    assert!(revoked.pending.is_empty());
    assert!(revoked.detail.is_some());
    assert_eq!(fake.fence.acl_restore("S-1", 5), None);
    Ok(())
}

#[test]
fn a_release_revokes_then_restores() -> TestResult {
    let fake = Fake::new(
        r#"case "$2" in
revoke) echo '[{"path":"a","status":"revoked"}]';;
restore) echo '{"paths":[{"path":"a","status":"restored"}],"parents":[]}';;
esac"#,
    )?;
    fake.fence.acl_release("S-1", 6)?;
    assert_eq!(&fake.argv()?[1..3], ["acl", "restore"]);
    Ok(())
}

#[test]
fn a_release_keeps_the_denies_when_a_grant_is_pending() -> TestResult {
    let fake = Fake::new(r#"echo '[{"path":"a","status":"leftChanged"}]'"#)?;
    let Err(error) = fake.fence.acl_release("S-1", 6) else {
        return Err("expected an error".into());
    };
    assert_eq!(
        error,
        AclReleaseError::GrantsNotReleased {
            pending: strings(&["a"]),
            detail: "no detail".to_string(),
        }
    );
    assert_eq!(
        error.to_string(),
        r#"the session grants were not all released, so the denies stay (pending ["a"], no detail)"#
    );
    assert_eq!(&fake.argv()?[1..3], ["acl", "revoke"]);
    Ok(())
}

#[test]
fn a_release_names_the_stderr_of_a_failed_revoke() -> TestResult {
    let fake = Fake::new("echo busy >&2; exit 1")?;
    let Err(AclReleaseError::GrantsNotReleased { pending, detail }) = fake.fence.acl_release("S-1", 6) else {
        return Err("expected GrantsNotReleased".into());
    };
    assert!(pending.is_empty());
    assert_eq!(detail, "busy");
    Ok(())
}

#[test]
fn a_release_reports_denies_that_could_not_be_restored() -> TestResult {
    let fake = Fake::new(
        r#"case "$2" in
revoke) echo '[]';;
restore) exit 1;;
esac"#,
    )?;
    let Err(error) = fake.fence.acl_release("S-1", 6) else {
        return Err("expected an error".into());
    };
    assert_eq!(error, AclReleaseError::DeniesNotRestored);
    assert_eq!(error.to_string(), "the session denies could not be restored");
    Ok(())
}
