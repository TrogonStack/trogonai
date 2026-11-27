#![cfg(any(target_os = "linux", target_os = "macos"))]

type TestResult = Result<(), Box<dyn std::error::Error>>;

use std::path::Path;

use trogon_sandbox_policy_exec::{
    CommandSpec, FilesystemView, FsPolicy, NetworkPolicy, PolicyExec, PolicyExecConfig, SandboxSettings,
};
use trogon_std::env::{ReadEnv, SystemEnv};

const WORKSPACE: &str = "TROGON_SANDBOX_POLICY_EXIT_WORKSPACE";
const ENDING: &str = "TROGON_SANDBOX_POLICY_EXIT_ENDING";
const CA_DIR: &str = "TROGON_SANDBOX_POLICY_EXIT_CA_DIR";

fn can_sandbox() -> bool {
    if !PolicyExec::is_supported_platform() {
        return false;
    }
    cfg!(not(target_os = "linux")) || std::process::Command::new("bwrap").arg("--version").output().is_ok()
}

fn leftovers(dir: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

#[tokio::test]
async fn child_runs_a_command_and_exits_without_resetting() -> TestResult {
    let Some(workspace) = SystemEnv.var_os(WORKSPACE) else {
        return Ok(());
    };
    let config = PolicyExecConfig::new(&workspace)
        .fs_policy(FsPolicy::default())
        .network(NetworkPolicy::DenyAll);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;
    let output = runtime.run(CommandSpec::shell("true")).await?;
    assert!(output.success(), "{}", output.stderr_text());
    std::process::exit(0)
}

#[test]
fn a_process_that_exits_straight_after_a_command_leaves_nothing_in_the_temp_directory() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tmp = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let status = std::process::Command::new(std::env::current_exe()?)
        .args([
            "child_runs_a_command_and_exits_without_resetting",
            "--exact",
            "--nocapture",
        ])
        .env(WORKSPACE, workspace.path())
        .env("TMPDIR", tmp.path())
        .status()?;
    assert!(status.success());
    assert_eq!(leftovers(tmp.path())?, Vec::<String>::new());
    Ok(())
}

fn masking_config(workspace: &Path) -> Result<PolicyExecConfig, Box<dyn std::error::Error>> {
    let token = workspace.join("token");
    std::fs::write(&token, "dummy\n")?;
    let tls = match SystemEnv.var_os(CA_DIR) {
        Some(dir) => format!(
            r#"{{"caCertPath": "{0}/ca.crt", "caKeyPath": "{0}/ca.key"}}"#,
            Path::new(&dir).display()
        ),
        None => "{}".to_string(),
    };
    let json = format!(
        r#"{{"network": {{"allowedDomains": ["example.com"], "deniedDomains": [],
            "tlsTerminate": {tls}}},
            "filesystem": {{"denyRead": [], "allowWrite": [], "denyWrite": []}},
            "credentials": {{"files": [{{"path": "{token}", "mode": "mask"}}]}}}}"#,
        token = token.display()
    );
    let config = SandboxSettings::from_json(&json)?
        .into_config(workspace)?
        .view(FilesystemView::Host);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    Ok(config)
}

#[tokio::test]
async fn child_masks_a_credential_file_behind_a_terminating_proxy_and_ends() -> TestResult {
    let Some(workspace) = SystemEnv.var_os(WORKSPACE) else {
        return Ok(());
    };
    let runtime = PolicyExec::new(masking_config(Path::new(&workspace))?)?;
    let output = runtime.run(CommandSpec::shell("true")).await?;
    assert!(output.success(), "{}", output.stderr_text());
    match SystemEnv.var(ENDING).as_deref() {
        Ok("reset-under-way") => {
            let _ = tokio::time::timeout(std::time::Duration::ZERO, runtime.reset()).await;
        }
        Ok("after-reset") => runtime.reset().await,
        _ => {}
    }
    std::process::exit(0)
}

fn run_child(test: &str, tmp: &Path, workspace: &Path, ending: &str, ca_dir: Option<&Path>) -> TestResult {
    let mut child = std::process::Command::new(std::env::current_exe()?);
    child
        .args([test, "--exact", "--nocapture"])
        .env(WORKSPACE, workspace)
        .env(ENDING, ending)
        .env("TMPDIR", tmp);
    if let Some(dir) = ca_dir {
        child.env(CA_DIR, dir);
    }
    assert!(child.status()?.success());
    Ok(())
}

#[test]
fn nothing_of_a_masked_credential_or_a_terminating_proxy_outlives_the_process() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    for ending in ["exit", "reset-under-way", "after-reset"] {
        let tmp = tempfile::tempdir()?;
        let workspace = tempfile::tempdir()?;
        run_child(
            "child_masks_a_credential_file_behind_a_terminating_proxy_and_ends",
            tmp.path(),
            workspace.path(),
            ending,
            None,
        )?;
        assert_eq!(leftovers(tmp.path())?, Vec::<String>::new(), "{ending}");
    }
    Ok(())
}

#[test]
fn a_ca_pair_the_configuration_names_survives_the_exit() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let tmp = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let ca = trogon_sandbox_policy_exec::generate_ca(None, None)?;
    let theirs = tmp.path().join("their-ca");
    std::fs::create_dir(&theirs)?;
    std::fs::write(theirs.join("ca.crt"), ca.cert_pem)?;
    std::fs::write(theirs.join("ca.key"), ca.key_pem)?;

    run_child(
        "child_masks_a_credential_file_behind_a_terminating_proxy_and_ends",
        tmp.path(),
        workspace.path(),
        "exit",
        Some(&theirs),
    )?;

    assert_eq!(leftovers(tmp.path())?, vec!["their-ca".to_string()]);
    assert_eq!(leftovers(&theirs)?, vec!["ca.crt", "ca.key"]);
    Ok(())
}
