#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, LiteralPath};
use trogon_sandbox_policy_exec::{
    CancellationToken, CommandOverrides, CommandSpec, CredentialOverride, FilesystemOverrides, FilesystemView,
    NetworkPolicy, PolicyExec, PolicyExecConfig, RuntimeError, SandboxSettings,
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
    std::fs::create_dir_all(root.join("other"))?;
    Ok((dir, root))
}

fn literal(path: &Path) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(path.to_str().ok_or("utf8 path")?)?,
        directory: DirectoryMarker::Unmarked,
    })
}

fn config(root: &Path, fs: FsPolicy) -> PolicyExecConfig {
    let config = PolicyExecConfig::new(root)
        .view(FilesystemView::Host)
        .network(NetworkPolicy::DenyAll)
        .fs_policy(fs);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    config
}

fn writes_to(root: &Path, dir: &str) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(FsPolicy::new(
        vec![],
        vec![],
        vec![],
        vec![],
        vec![literal(&root.join(dir))?],
        vec![],
    )?)
}

fn touch(root: &Path, dir: &str) -> CommandSpec {
    CommandSpec::shell(format!("echo x > {}/{dir}/f.txt", root.display()))
}

#[tokio::test]
async fn a_command_can_replace_the_filesystem_policy_for_itself() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(config(&root, writes_to(&root, "out")?))?;

    let own = runtime.run(touch(&root, "other")).await?;
    assert!(!own.success());
    assert!(!root.join("other/f.txt").exists());

    let overridden = runtime
        .run(touch(&root, "other").fs_policy(writes_to(&root, "other")?))
        .await?;
    assert!(overridden.success(), "{}", overridden.stderr_text());
    assert!(root.join("other/f.txt").exists());

    let kept = runtime.run(touch(&root, "out")).await?;
    assert!(kept.success(), "{}", kept.stderr_text());
    Ok(())
}

#[tokio::test]
async fn a_command_override_replaces_only_the_lists_it_sets() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(config(&root, writes_to(&root, "out")?))?;
    let frozen = literal(&root.join("out"))?;

    let denied =
        runtime
            .run(touch(&root, "out").overrides(
                CommandOverrides::default().filesystem(FilesystemOverrides::default().deny_write(vec![frozen])),
            ))
            .await?;
    assert!(!denied.success());
    assert!(!root.join("out/f.txt").exists());

    let kept = runtime
        .run(touch(&root, "out").overrides(CommandOverrides::default().filesystem(FilesystemOverrides::default())))
        .await?;
    assert!(kept.success(), "{}", kept.stderr_text());
    Ok(())
}

#[tokio::test]
async fn a_command_override_can_turn_the_filesystem_rules_off_for_itself() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(config(&root, writes_to(&root, "out")?))?;
    let open = runtime
        .run(
            touch(&root, "other")
                .overrides(CommandOverrides::default().filesystem(FilesystemOverrides::default().disabled(true))),
        )
        .await?;
    assert!(open.success(), "{}", open.stderr_text());
    assert!(root.join("other/f.txt").exists());
    Ok(())
}

#[tokio::test]
async fn a_command_override_can_deny_a_credential_file_for_itself() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let secret = root.join("other/secret.txt");
    std::fs::write(&secret, "token")?;
    let json = format!(
        r#"{{"network": {{"allowedDomains": [], "deniedDomains": []}},
            "filesystem": {{"denyRead": [], "allowWrite": ["{out}"], "denyWrite": []}}}}"#,
        out = root.join("out").display()
    );
    let config = SandboxSettings::from_json(&json)?
        .into_config(&root)?
        .view(FilesystemView::Host);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;
    let read = || CommandSpec::shell(format!("cat {}", secret.display()));

    let open = runtime.run(read()).await?;
    assert!(open.success(), "{}", open.stderr_text());

    let own = CredentialOverride::from_json(&format!(
        r#"{{"files": [{{"path": "{}", "mode": "deny"}}]}}"#,
        secret.display()
    ))?;
    let denied = runtime
        .run(read().overrides(CommandOverrides::default().credentials(own)))
        .await?;
    assert!(!denied.success());

    let again = runtime.run(read()).await?;
    assert!(again.success(), "{}", again.stderr_text());
    Ok(())
}

#[tokio::test]
async fn cancelling_before_the_start_refuses_the_command() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(config(&root, FsPolicy::default()))?;
    let token = CancellationToken::new();
    token.cancel();
    let result = runtime.run(CommandSpec::shell("true").cancel_on(token)).await;
    assert!(matches!(result, Err(RuntimeError::Cancelled)));
    Ok(())
}

#[tokio::test]
async fn cancelling_while_running_stops_the_command() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(config(&root, FsPolicy::default()))?;
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        canceller.cancel();
    });
    let started = std::time::Instant::now();
    let output = runtime.run(CommandSpec::shell("sleep 30").cancel_on(token)).await?;
    assert!(!output.success());
    assert!(started.elapsed() < Duration::from_secs(20));
    Ok(())
}

#[tokio::test]
async fn update_config_replaces_the_policy_and_refuses_a_fixed_part() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(config(&root, writes_to(&root, "out")?))?;
    assert!(runtime.is_sandboxing_enabled());

    runtime.update_config(config(&root, writes_to(&root, "other")?)).await?;
    let now = runtime.config().await;
    assert!(now.fs_write_restriction().is_some());

    let moved = runtime.run(touch(&root, "other")).await?;
    assert!(moved.success(), "{}", moved.stderr_text());
    let old = runtime.run(touch(&root, "out")).await?;
    assert!(!old.success());

    let (_second, elsewhere) = scratch()?;
    let result = runtime
        .update_config(config(&elsewhere, writes_to(&elsewhere, "out")?))
        .await;
    assert!(matches!(result, Err(RuntimeError::Invalid(_))));
    let still = runtime.run(touch(&root, "other")).await?;
    assert!(still.success(), "{}", still.stderr_text());
    Ok(())
}

#[tokio::test]
async fn turning_the_violation_monitor_off_records_nothing() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::builder(config(&root, writes_to(&root, "out")?))
        .violation_monitor(false)
        .build()?;
    let output = runtime.run(touch(&root, "other")).await?;
    assert!(!output.success());
    assert!(runtime.violations().violations(None).is_empty());
    Ok(())
}

fn disabled_runtime(root: &Path, secret: &Path, disabled: bool) -> Result<PolicyExec, Box<dyn std::error::Error>> {
    let json = format!(
        r#"{{"network": {{"allowedDomains": [], "deniedDomains": []}},
            "filesystem": {{"disabled": {disabled}, "denyRead": ["{secret}"], "allowWrite": [], "denyWrite": []}}}}"#,
        secret = secret.display()
    );
    let config = SandboxSettings::from_json(&json)?
        .into_config(root)?
        .view(FilesystemView::Host);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    Ok(PolicyExec::new(config)?)
}

#[tokio::test]
async fn a_disabled_filesystem_reports_no_enforcement() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let secret = root.join("other/secret.txt");
    let runtime = disabled_runtime(&root, &secret, true)?;
    let config = runtime.config().await;

    let read = config.fs_read_restriction().ok_or("no read restriction")?;
    assert!(read.deny_only.is_empty());
    assert!(read.allow_within_deny.is_empty());
    let write = config.fs_write_restriction().ok_or("no write restriction")?;
    assert_eq!(write.allow_only, vec![literal(Path::new("/"))?]);
    assert!(write.deny_within_allow.is_empty());

    let enforced = disabled_runtime(&root, &secret, false)?.config().await;
    assert_eq!(
        enforced
            .fs_read_restriction()
            .ok_or("no read restriction")?
            .deny_only
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn deny_read_is_ignored_when_the_filesystem_is_disabled() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let secret = root.join("other/secret.txt");
    std::fs::write(&secret, "token")?;
    let read = || CommandSpec::shell(format!("cat {}", secret.display()));

    let off = disabled_runtime(&root, &secret, true)?.run(read()).await?;
    assert!(off.success(), "{}", off.stderr_text());
    assert_eq!(off.stdout_text(), "token");

    let on = disabled_runtime(&root, &secret, false)?.run(read()).await?;
    assert!(!on.success());
    Ok(())
}

#[tokio::test]
async fn tmpdir_is_left_alone_when_the_filesystem_is_disabled_and_set_otherwise() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let secret = root.join("other/secret.txt");
    let print = || CommandSpec::shell("printf %s \"${TMPDIR-unset}\"");
    let inherited = SystemEnv.var("TMPDIR").unwrap_or_else(|_| "unset".to_string());
    let tmpdir_vars = ["TROGON_SANDBOX_POLICY_TMPDIR", "TROGON_SANDBOX_TMPDIR"]
        .iter()
        .find_map(|name| SystemEnv.var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "/tmp/sandbox".to_string());

    let off = disabled_runtime(&root, &secret, true)?.run(print()).await?;
    assert_eq!(off.stdout_text(), inherited);

    let on = disabled_runtime(&root, &secret, false)?.run(print()).await?;
    assert_eq!(on.stdout_text(), tmpdir_vars);
    Ok(())
}
