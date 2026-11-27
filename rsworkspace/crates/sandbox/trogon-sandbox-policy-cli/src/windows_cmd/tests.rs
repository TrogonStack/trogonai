use std::fs;
use std::os::unix::fs::PermissionsExt;

use trogon_sandbox_windows_fence::FenceHelper;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// An environment with no variables set.
struct NoEnv;

impl ReadEnv for NoEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn usage(error: UsageError) -> String {
    error.0
}

#[test]
fn install_options_parse() -> TestResult {
    let opts = parse_install(&strings(&[
        "--sublayer-guid",
        "g",
        "--proxy-port-range=60000-60003",
        "--allow-loopback",
        "--sandbox-user",
        "boxed",
        "--force",
        "--profile-exclude",
        "a",
        "--profile-exclude=b",
        "--skip-profile-grant",
    ]))
    .map_err(usage)?;
    assert_eq!(opts.sublayer_guid.ok_or("no guid")?.as_str(), "g");
    assert_eq!(opts.proxy_port_range.ok_or("no range")?.to_string(), "60000-60003");
    assert_eq!(opts.sandbox_user.ok_or("no user")?.as_str(), "boxed");
    assert!(opts.allow_loopback && opts.force && opts.skip_profile_grant);
    assert_eq!(opts.profile_exclude, ["a", "b"]);
    Ok(())
}

#[test]
fn bad_input_is_a_usage_error() -> TestResult {
    assert!(parse_install(&strings(&["--nope"])).is_err());
    assert!(parse_install(&strings(&["--sandbox-user"])).is_err());
    assert!(parse_install(&strings(&["--proxy-port-range", "9-1"])).is_err());
    assert!(parse_uninstall(&strings(&["--force"])).is_err());
    let uninstall = parse_uninstall(&strings(&["--sublayer-guid=x"])).map_err(usage)?;
    assert_eq!(uninstall.sublayer_guid.ok_or("no guid")?.as_str(), "x");
    Ok(())
}

fn fake_fence(dir: &tempfile::TempDir, script: &str) -> Result<WindowsFence, Box<dyn std::error::Error>> {
    let exe = dir.path().join("trogon-sandbox-fence-helper");
    fs::write(&exe, format!("#!/bin/sh\nshift\n{script}\n"))?;
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))?;
    wait_until_executable(&exe)?;
    Ok(WindowsFence::new(FenceHelper::new(&exe)?))
}

// A child another test forks while the script is still open for writing keeps that
// descriptor until it execs, and exec of the script fails with ETXTBSY until then.
fn wait_until_executable(exe: &std::path::Path) -> TestResult {
    for _ in 0..200 {
        match std::process::Command::new(exe).arg("__probe").output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            Err(e) => return Err(format!("probing {}: {e}", exe.display()).into()),
            Ok(_) => return Ok(()),
        }
    }
    Err(format!("{} stayed busy", exe.display()).into())
}

#[test]
fn uninstall_messages_and_codes() -> TestResult {
    for (exit, code, out_text, err_text) in [
        (0, 0, "WFP filters and `trogon-sandbox` account removed.\n", ""),
        (10, 2, "", "Uninstall cancelled at the UAC prompt.\n"),
        (1, 1, "", "Error: fence-helper uninstall failed (exit 1): boom\n"),
    ] {
        let dir = tempfile::tempdir()?;
        let fence = fake_fence(&dir, &format!("echo boom >&2; exit {exit}"))?;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let got = run_uninstall(&fence, &UninstallOptions::default(), &mut out, &mut err);
        assert_eq!(got, code);
        assert_eq!(String::from_utf8(out)?, out_text);
        assert_eq!(String::from_utf8(err)?, err_text);
    }
    Ok(())
}

#[test]
fn install_cancelled_message() -> TestResult {
    let dir = tempfile::tempdir()?;
    let fence = fake_fence(
        &dir,
        r#"case "$1" in install) exit 10;; status) echo '{"user":{"user":{"exists":false,"group_exists":false,"in_builtin_users":false,"in_sandbox_group":false,"hidden_from_logon":false},"cred_present":false,"real_user_sid":"S-1-5-21-1","session_user_sid":"S-1-5-21-1"},"wfp":{"state":"absent","filters":0}}';; esac"#,
    )?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let got = run_install(&fence, &InstallOptions::default(), &NoEnv, &mut out, &mut err);
    let err = String::from_utf8(err)?;
    assert_eq!(got, 2, "{err}");
    assert_eq!(err, "Install cancelled at the UAC prompt. Nothing changed.\n");
    Ok(())
}
