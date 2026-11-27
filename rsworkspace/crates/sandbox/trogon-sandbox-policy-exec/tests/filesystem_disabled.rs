#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::path::{Path, PathBuf};

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, LiteralPath};
use trogon_sandbox_policy_exec::{
    CommandSpec, FilesystemView, FsReadRestriction, FsWriteRestriction, NetworkPolicy, PolicyExec, PolicyExecConfig,
    SandboxSettings,
};
use trogon_std::env::{ReadEnv, SystemEnv};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn can_sandbox() -> bool {
    if !PolicyExec::is_supported_platform() {
        return false;
    }
    cfg!(not(target_os = "linux")) || std::process::Command::new("bwrap").arg("--version").output().is_ok()
}

fn scratch() -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    std::fs::create_dir_all(root.join("out"))?;
    Ok((dir, root))
}

fn config(root: &Path, disabled: bool) -> Result<PolicyExecConfig, Box<dyn std::error::Error>> {
    let json = format!(
        r#"{{"network": {{"allowedDomains": [], "deniedDomains": []}},
            "filesystem": {{"denyRead": ["{secret}"], "allowWrite": ["{out}"],
            "denyWrite": [], "disabled": {disabled}}}}}"#,
        secret = root.join("secret.txt").display(),
        out = root.join("out").display(),
    );
    let config = SandboxSettings::from_json(&json)?
        .into_config(root)?
        .view(FilesystemView::Host)
        .network(NetworkPolicy::DenyAll);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    Ok(config)
}

#[test]
fn the_getters_report_no_enforcement_when_the_rules_are_off() -> TestResult {
    let (_dir, root) = scratch()?;

    let off = config(&root, true)?;
    assert_eq!(off.fs_read_restriction(), Some(FsReadRestriction::default()));
    assert_eq!(
        off.fs_write_restriction(),
        Some(FsWriteRestriction {
            allow_only: vec![FsPathEntry::Literal {
                path: LiteralPath::new("/")?,
                directory: DirectoryMarker::Unmarked,
            }],
            deny_within_allow: vec![],
        })
    );

    let on = config(&root, false)?;
    let on_read = on.fs_read_restriction().ok_or("no read restriction")?;
    assert!(!on_read.deny_only.is_empty());
    assert_ne!(on.fs_write_restriction(), off.fs_write_restriction());
    Ok(())
}

#[tokio::test]
async fn a_read_deny_is_ignored_when_the_rules_are_off_and_kept_when_they_are_on() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    std::fs::write(root.join("secret.txt"), "classified")?;
    let read = || CommandSpec::shell(format!("cat {}/secret.txt", root.display()));

    let off = PolicyExec::new(config(&root, true)?)?;
    let open = off.run(read()).await?;
    assert!(open.success(), "{}", open.stderr_text());
    assert_eq!(open.stdout_text(), "classified");

    let on = PolicyExec::new(config(&root, false)?)?;
    let denied = on.run(read()).await?;
    assert!(!denied.success());
    assert!(!denied.stdout_text().contains("classified"));
    Ok(())
}

#[tokio::test]
async fn tmpdir_is_left_as_the_host_has_it_when_the_rules_are_off_and_handed_out_when_on() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let host = SystemEnv.var("TMPDIR").unwrap_or_default();
    let handed_out = ["TROGON_SANDBOX_POLICY_TMPDIR", "TROGON_SANDBOX_TMPDIR"]
        .iter()
        .find_map(|name| SystemEnv.var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "/tmp/sandbox".to_string());
    let show = || CommandSpec::shell("printf %s \"$TMPDIR\"");

    let off = PolicyExec::new(config(&root, true)?)?;
    let kept = off.run(show()).await?;
    assert!(kept.success(), "{}", kept.stderr_text());
    assert_eq!(kept.stdout_text(), host);

    let on = PolicyExec::new(config(&root, false)?)?;
    let set = on.run(show()).await?;
    assert_eq!(set.stdout_text(), handed_out);
    Ok(())
}
