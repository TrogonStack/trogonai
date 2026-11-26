#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use trogon_sandbox_windows_fence::{
    DEFAULT_PROXY_PORT_RANGE, FenceHelper, InstallOptions, InstallOutcome, InstallProfileGrant, InstallProgress,
    NoProgress, SublayerGuid, UninstallOptions, UninstallOutcome, VerifyOptions, WindowsFence,
    WindowsSandboxErrorCode as Code,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A stand-in `fence-helper`: a shell script that records its argv and prints
/// what the test hands it.
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
                "#!/bin/sh\n[ \"$1\" = __probe ] && exit 0\nprintf '%s\\n' \"$@\" > \"{}/argv\"\nshift\n{script}\n",
                dir.path().display()
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
}

// A child another test forks while the script is still open for writing keeps that
// descriptor until it execs, and exec of the script fails with ETXTBSY until then.
fn wait_until_executable(exe: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..200 {
        match std::process::Command::new(exe).arg("__probe").status() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(format!("probing {}: {e}", exe.display()).into()),
            Ok(_) => return Ok(()),
        }
    }
    Err(format!("{} stayed busy", exe.display()).into())
}

const STATUS_JSON: &str = r#"{"user":{"user":{"exists":true,"sid":"S-1-5-21-9","group_exists":true,"in_builtin_users":false,"in_sandbox_group":true,"hidden_from_logon":true},"cred_present":true,"real_user_sid":"S-1-5-21-1","session_user_sid":"S-1-5-21-1"},"wfp":{"state":"installed","filters":6,"port_range":[60080,60089]}}"#;
const GRANT_JSON: &str = r#"{"profile":"P","containers":1,"granted":2,"files":3,"denied":4,"placeholders":0,"writes":0,"unchanged":0,"removed":0,"elapsed_ms":5}"#;

fn script_for_install(exit: i32) -> String {
    format!(
        "case \"$1\" in\n install) echo 'install said' >&2; exit {exit};;\n status) echo '{STATUS_JSON}';;\n profile) echo 'half done' >&2; echo '{GRANT_JSON}';;\nesac"
    )
}

#[derive(Default)]
struct Recorder {
    notices: Vec<String>,
    lines: Vec<String>,
}

impl InstallProgress for Recorder {
    fn profile_grant_start(&mut self, notice: &str) {
        self.notices.push(notice.to_string());
    }
    fn profile_grant_line(&mut self, line: &str) {
        self.lines.push(line.to_string());
    }
}

#[test]
fn missing_executable_is_typed() -> TestResult {
    let Err(e) = FenceHelper::new("/definitely/not/here") else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperNotFound);
    Ok(())
}

#[test]
fn dispatch_argument_comes_first() -> TestResult {
    let fake = Fake::new(&format!("echo '{STATUS_JSON}'"))?;
    fake.fence.status(Some(&SublayerGuid::new("g")?))?;
    assert_eq!(fake.argv()?, ["--fence-helper", "status", "--sublayer-guid", "g"]);
    Ok(())
}

#[test]
fn status_nonzero_is_typed() -> TestResult {
    let fake = Fake::new("echo oops >&2; exit 7")?;
    let Err(e) = fake.fence.status(None) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperNonzero);
    assert_eq!(e.message(), "fence-helper status exited 7: oops");
    Ok(())
}

#[test]
fn timeout_kills_and_is_typed() -> TestResult {
    let fake = Fake::new("sleep 5")?;
    let Err(e) = fake
        .fence
        .fence_helper()
        .run(&["status".to_string()], Duration::from_millis(150))
    else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperTimeout);
    assert_eq!(e.message(), "fence-helper status timed out after 150ms");
    Ok(())
}

#[test]
fn a_timeout_of_zero_is_no_limit() -> TestResult {
    let fake = Fake::new("sleep 0.3; echo ok")?;
    let out = fake.fence.fence_helper().run(&["status".to_string()], Duration::ZERO)?;
    assert_eq!(out.stdout, "ok");
    Ok(())
}

#[test]
fn install_applies_profile_grant_and_streams_progress() -> TestResult {
    let fake = Fake::new(&script_for_install(0))?;
    let mut rec = Recorder::default();
    let outcome = fake.fence.install(&InstallOptions::default(), &mut rec)?;
    let InstallOutcome::Installed { status, profile_grant } = outcome else {
        return Err("expected an install".into());
    };
    assert_eq!(status.wfp.filters, 6);
    assert!(matches!(profile_grant, InstallProfileGrant::Applied { ref result, .. } if result.granted == 2));
    assert_eq!(rec.notices.len(), 1);
    assert_eq!(rec.lines, ["half done"]);
    assert_eq!(fake.argv()?, ["--fence-helper", "profile", "grant", "--json"]);
    Ok(())
}

#[test]
fn install_can_skip_the_grant() -> TestResult {
    let fake = Fake::new(&script_for_install(0))?;
    let opts = InstallOptions {
        skip_profile_grant: true,
        ..InstallOptions::default()
    };
    let outcome = fake.fence.install(&opts, &mut NoProgress)?;
    assert!(matches!(
        outcome,
        InstallOutcome::Installed { profile_grant: InstallProfileGrant::Skipped { ref reason }, .. }
            if reason == "--skip-profile-grant was set"
    ));
    assert_eq!(fake.argv()?, ["--fence-helper", "status"]);
    Ok(())
}

#[test]
fn install_cancelled_at_uac() -> TestResult {
    let fake = Fake::new(&script_for_install(10))?;
    let outcome = fake.fence.install(&InstallOptions::default(), &mut NoProgress)?;
    assert_eq!(outcome, InstallOutcome::Cancelled);
    Ok(())
}

#[test]
fn install_failure_exit_is_mapped() -> TestResult {
    let fake = Fake::new(&script_for_install(13))?;
    let Err(e) = fake.fence.install(&InstallOptions::default(), &mut NoProgress) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::InstallConfigConflict);
    assert!(e.message().ends_with("Output: install said"));
    Ok(())
}

#[test]
fn install_timeout_is_remapped() -> TestResult {
    let fake = Fake::new("sleep 5")?;
    let opts = InstallOptions {
        timeout_ms: Some(150),
        ..InstallOptions::default()
    };
    let Err(e) = fake.fence.install(&opts, &mut NoProgress) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::InstallTimeout);
    Ok(())
}

#[test]
fn profile_grant_failure_keeps_install_message() -> TestResult {
    let script = format!(
        "case \"$1\" in\n install) exit 0;;\n status) echo '{STATUS_JSON}';;\n profile) echo 'denied' >&2; exit 1;;\nesac"
    );
    let fake = Fake::new(&script)?;
    let Err(e) = fake.fence.install(&InstallOptions::default(), &mut NoProgress) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::ProfileGrantFailed);
    assert!(e.message().starts_with("The Windows sandbox is installed, but the read grant on your user profile could not be applied: fence-helper profile grant exited 1: denied"));
    Ok(())
}

#[test]
fn uninstall_outcomes() -> TestResult {
    for (exit, expect) in [
        (0, Some(UninstallOutcome::Removed)),
        (10, Some(UninstallOutcome::Cancelled)),
        (1, None),
    ] {
        let fake = Fake::new(&format!("echo gone >&2; exit {exit}"))?;
        let r = fake.fence.uninstall(&UninstallOptions::default());
        match (expect, r) {
            (Some(o), Ok(got)) => assert_eq!(got, o),
            (None, Err(e)) => {
                assert_eq!(e.code(), Code::UninstallFailed);
                assert_eq!(e.message(), "fence-helper uninstall failed (exit 1): gone");
            }
            _ => return Err("unexpected uninstall result".into()),
        }
    }
    Ok(())
}

#[test]
fn refusals_beside_others_surface_from_any_subcommand() -> TestResult {
    let fake = Fake::new("echo '{\"held_by\":[{\"pid\":4,\"kind\":\"profile\",\"path\":\"x\"}]}'; exit 22")?;
    let Err(e) = fake.fence.uninstall(&UninstallOptions::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::KeptGrantsInUse);
    assert!(e.message().contains("process 4 is a sandbox session or command"));
    Ok(())
}

#[test]
fn verify_binds_outside_the_range_and_passes_flags() -> TestResult {
    let fake = Fake::new(
        r#"echo '{"egress_probe":"blocked","target":"t","low_integrity_probe":"blocked","untrusted_integrity_probe":"blocked"}'"#,
    )?;
    let opts = VerifyOptions {
        allow_loopback: true,
        write_roots: vec!["C:\\w".to_string()],
        ..VerifyOptions::default()
    };
    let r = fake.fence.verify(&opts)?;
    assert_eq!(r.target, "t");
    let argv = fake.argv()?;
    assert_eq!(&argv[..4], ["--fence-helper", "wfp", "verify", "--target"]);
    let port: u16 = argv[4].rsplit(':').next().ok_or("no port")?.parse()?;
    assert!(!DEFAULT_PROXY_PORT_RANGE.contains(port));
    assert_eq!(
        &argv[5..],
        ["--allow-loopback", "--write-restricted-token", "--write-root", "C:\\w"]
    );
    Ok(())
}

#[test]
fn verify_reports_a_disabled_seclogon_before_parsing() -> TestResult {
    let fake = Fake::new(r#"echo '{"code":"seclogon_disabled","state":"disabled"}' >&2; exit 18"#)?;
    let Err(e) = fake.fence.verify(&VerifyOptions::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::SeclogonDisabled);
    Ok(())
}

#[test]
fn trust_ca_passes_the_path_and_reports_a_failure() -> TestResult {
    let fake = Fake::new("exit 0")?;
    fake.fence.trust_ca(Path::new("C:\\ca.pem"))?;
    assert_eq!(fake.argv()?, ["--fence-helper", "user", "trust-ca", "C:\\ca.pem"]);
    let fake = Fake::new("echo bad cert >&2; exit 3")?;
    let Err(e) = fake.fence.trust_ca(Path::new("x.pem")) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::TrustCaFailed);
    assert_eq!(
        e.message(),
        "fence-helper user trust-ca 'x.pem' failed (exit 3): bad cert"
    );
    Ok(())
}

#[test]
fn trust_ca_reports_a_disabled_seclogon() -> TestResult {
    let fake = Fake::new(r#"echo '{"code":"seclogon_disabled","state":"missing"}' >&2; exit 18"#)?;
    let Err(e) = fake.fence.trust_ca(Path::new("x.pem")) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::SeclogonDisabled);
    Ok(())
}

#[test]
fn verify_explicit_target_skips_the_listener() -> TestResult {
    let fake = Fake::new(r#"echo '{"egress_probe":"connected","target":"1.2.3.4:5"}'; exit 3"#)?;
    let opts = VerifyOptions {
        target: Some("1.2.3.4:5".to_string()),
        ..VerifyOptions::default()
    };
    let Err(e) = fake.fence.verify(&opts) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpFenceInactive);
    assert_eq!(fake.argv()?[4], "1.2.3.4:5");
    Ok(())
}

#[test]
fn profile_forget_error() -> TestResult {
    let fake = Fake::new("echo nope >&2; exit 2")?;
    let Err(e) = fake.fence.profile_forget(None) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::ProfileForgetFailed);
    assert!(e.message().ends_with("exited 2): nope"));
    Ok(())
}

#[test]
fn the_profile_read_status_asks_the_helper_and_parses_the_answer() -> TestResult {
    let fake = Fake::new("echo '{\"state\":\"none\"}'")?;
    let status = fake.fence.profile_read_status()?;
    assert_eq!(status.state.as_str(), "none");
    assert_eq!(fake.argv()?, vec!["--fence-helper", "profile", "status"]);
    Ok(())
}

#[test]
fn the_profile_read_status_can_ask_for_paths() -> TestResult {
    let fake = Fake::new("echo '{\"state\":\"none\"}'")?;
    fake.fence.profile_read_status_with(true)?;
    assert_eq!(fake.argv()?.last().map(String::as_str), Some("--paths"));
    Ok(())
}

#[test]
fn a_profile_operation_in_progress_is_an_error_not_a_status() -> TestResult {
    let fake = Fake::new("echo '{\"state\":\"in_progress\"}'")?;
    let Err(error) = fake.fence.profile_read_status() else {
        return Err("expected an error".into());
    };
    assert_eq!(error.code(), Code::ProfileOperationInProgress);
    Ok(())
}
