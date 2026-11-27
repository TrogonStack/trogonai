#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, LiteralPath};
use trogon_sandbox_policy_exec::{
    CommandId, CommandSpec, FilesystemView, NetworkPolicy, PolicyExec, PolicyExecConfig, RuntimeError, StdioMode,
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
    std::fs::write(root.join("readable.txt"), "visible\n")?;
    Ok((dir, root))
}

fn host_config(root: &Path, fs: Option<FsPolicy>) -> PolicyExecConfig {
    let config = PolicyExecConfig::new(root)
        .view(FilesystemView::Host)
        .network(NetworkPolicy::DenyAll);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    match fs {
        Some(fs) => config.fs_policy(fs),
        None => config,
    }
}

fn write_policy(root: &Path) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    let out = FsPathEntry::Literal {
        path: LiteralPath::new(root.join("out").to_str().ok_or("utf8 path")?)?,
        directory: DirectoryMarker::Unmarked,
    };
    Ok(FsPolicy::new(vec![], vec![], vec![], vec![], vec![out], vec![])?)
}

#[tokio::test]
async fn the_host_view_reads_host_paths_and_writes_only_what_is_allowed() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(host_config(&root, Some(write_policy(&root)?)))?;

    let allowed = runtime
        .run(CommandSpec::shell(format!(
            "cat {0}/readable.txt > {0}/out/copy.txt",
            root.display()
        )))
        .await?;
    assert!(allowed.success(), "{}", allowed.stderr_text());
    assert_eq!(std::fs::read_to_string(root.join("out/copy.txt"))?, "visible\n");

    let denied = runtime
        .run(CommandSpec::shell(format!("echo no > {}/outside.txt", root.display())))
        .await?;
    assert!(!denied.success());
    assert!(!root.join("outside.txt").exists());
    Ok(())
}

#[tokio::test]
async fn an_empty_host_policy_reads_everything_and_writes_nothing() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(host_config(&root, Some(FsPolicy::default())))?;

    let read = runtime
        .run(CommandSpec::shell(format!("cat {}/readable.txt", root.display())))
        .await?;
    assert!(read.success(), "{}", read.stderr_text());
    assert_eq!(read.stdout_text(), "visible\n");

    let write = runtime
        .run(CommandSpec::shell(format!("echo no > {}/out/x.txt", root.display())))
        .await?;
    assert!(!write.success());
    assert!(!root.join("out/x.txt").exists());
    Ok(())
}

#[tokio::test]
async fn the_host_view_runs_in_the_given_directory_with_the_inherited_environment() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(host_config(&root, None))?;

    let here = runtime.run(CommandSpec::shell("pwd").cwd(root.join("out"))).await?;
    assert_eq!(here.stdout_text().trim(), root.join("out").to_str().ok_or("utf8 path")?);

    let default = runtime.run(CommandSpec::shell("pwd")).await?;
    assert_eq!(default.stdout_text().trim(), root.to_str().ok_or("utf8 path")?);

    let env = runtime
        .run(CommandSpec::shell("printf '%s|%s' \"$TROGON_SANDBOXED\" \"$PATH\""))
        .await?;
    assert_eq!(env.stdout_text(), format!("1|{}", SystemEnv.var("PATH")?));
    Ok(())
}

#[tokio::test]
async fn the_guest_view_refuses_a_working_directory() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let config = PolicyExecConfig::new(&root).network(NetworkPolicy::DenyAll);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;
    let refused = runtime.run(CommandSpec::shell("pwd").cwd(&root)).await;
    assert!(matches!(refused, Err(RuntimeError::Unsupported(_))));
    Ok(())
}

#[tokio::test]
async fn a_spawned_command_hands_over_its_pipes() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(host_config(&root, None))?;
    let mut running = runtime
        .spawn(
            CommandSpec::shell("read line; echo got:$line")
                .stdin(StdioMode::Piped)
                .stdout(StdioMode::Piped)
                .stderr(StdioMode::Null),
        )
        .await?;
    assert!(running.pid().is_some());
    let mut stdin = running.take_stdin().ok_or("stdin")?;
    let mut stdout = running.take_stdout().ok_or("stdout")?;
    stdin.write_all(b"hello\n").await?;
    drop(stdin);
    let mut text = String::new();
    stdout.read_to_string(&mut text).await?;
    assert_eq!(text, "got:hello\n");
    let exit = running.wait().await?;
    assert!(exit.status.success());
    assert!(!exit.timed_out);
    Ok(())
}

#[tokio::test]
async fn a_timeout_kills_the_command() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(host_config(&root, None))?;

    let output = runtime
        .run(CommandSpec::shell("sleep 30").timeout(Duration::from_millis(500)))
        .await?;
    assert!(output.timed_out);
    assert!(!output.success());

    let running = runtime
        .spawn(
            CommandSpec::shell("sleep 30")
                .stdout(StdioMode::Null)
                .stderr(StdioMode::Null),
        )
        .await?;
    running.kill();
    let exit = running.wait().await?;
    assert!(!exit.status.success());
    assert!(!exit.timed_out);
    Ok(())
}

#[tokio::test]
async fn a_caller_supplied_command_id_names_the_command() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(host_config(&root, None))?;

    let id = CommandId::new("0123456789abcdefghijklmnop");
    let output = runtime.run(CommandSpec::shell("true").command_id(id.clone())).await?;
    assert_eq!(output.command, id);

    let short = runtime
        .run(CommandSpec::shell("true").command_id(CommandId::new("short")))
        .await;
    assert!(matches!(short, Err(RuntimeError::Invalid(_))));
    Ok(())
}

#[tokio::test]
async fn the_host_view_hands_the_command_the_policy_tmpdir_and_writes_nothing_else_for_it() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let expected = ["TROGON_SANDBOX_POLICY_TMPDIR", "TROGON_SANDBOX_TMPDIR"]
        .iter()
        .find_map(|name| SystemEnv.var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "/tmp/sandbox".to_string());
    let (_dir, root) = scratch()?;
    let runtime = PolicyExec::new(host_config(&root, Some(write_policy(&root)?)))?;

    let output = runtime.run(CommandSpec::shell("printf %s \"$TMPDIR\"")).await?;

    assert!(output.success(), "{}", output.stderr_text());
    assert_eq!(output.stdout_text(), expected);
    Ok(())
}
