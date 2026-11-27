#![cfg(any(target_os = "linux", target_os = "macos"))]

type TestResult = Result<(), Box<dyn std::error::Error>>;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use trogon_sandbox_contract::DirectoryMarker;
use trogon_sandbox_policy_exec::{
    AllowRange, AskQuestion, AskVerdict, CommandSpec, DenialReason, FsPathEntry, FsPolicy, LiteralPath, NetworkPolicy,
    OutputLimit, PolicyExec, PolicyExecConfig, StdioMode,
};

const ALLOWED_HOST: &str = "example.com";
const DENIED_HOST: &str = "denied.example.com";
const DENIAL_REASON: &str = "not on the allow list";

fn tool_available(tool: &str) -> bool {
    std::process::Command::new(tool).arg("--version").output().is_ok()
}

fn can_sandbox() -> bool {
    if !PolicyExec::is_supported_platform() || !tool_available("curl") {
        return false;
    }
    cfg!(not(target_os = "linux")) || tool_available("bwrap")
}

fn internet_reachable() -> bool {
    std::process::Command::new("curl")
        .args(["-fsS", "-o", "/dev/null", "--max-time", "5"])
        .arg(format!("http://{ALLOWED_HOST}/"))
        .status()
        .is_ok_and(|status| status.success())
}

fn fs_policy() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    let entry = |raw: &str| -> Result<FsPathEntry, Box<dyn std::error::Error>> {
        Ok(FsPathEntry::Literal {
            path: LiteralPath::new(raw)?,
            directory: DirectoryMarker::Unmarked,
        })
    };
    Ok(FsPolicy::new(
        vec![],
        vec![],
        vec![],
        vec![],
        vec![entry("/workspace/out")?],
        vec![entry("/workspace/out/locked")?],
    )?)
}

fn loopback() -> Result<AllowRange, Box<dyn std::error::Error>> {
    Ok(AllowRange::new("127.0.0.0/8")?)
}

fn user_defined(allow_ranges: Vec<AllowRange>) -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges,
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    }
}

#[cfg(target_os = "macos")]
fn guest_path(workspace: &Path, relative: &str) -> String {
    workspace.join(relative).to_string_lossy().into_owned()
}

#[cfg(target_os = "linux")]
fn guest_path(_workspace: &Path, relative: &str) -> String {
    format!("/workspace/{relative}")
}

fn serve_ok() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    Ok(port)
}

fn config(workspace: &Path, network: NetworkPolicy) -> Result<PolicyExecConfig, Box<dyn std::error::Error>> {
    let config = PolicyExecConfig::new(workspace)
        .fs_policy(fs_policy()?)
        .network(network);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    Ok(config)
}

fn workspace() -> Result<(tempfile::TempDir, std::path::PathBuf), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let workspace = dir.path().canonicalize()?;
    std::fs::create_dir_all(workspace.join("out/locked"))?;
    Ok((dir, workspace))
}

fn curl(url: &str) -> CommandSpec {
    CommandSpec::shell(format!("curl -fsS --max-time 20 {url}"))
}

#[tokio::test]
async fn filesystem_policy_allows_one_write_and_denies_another() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let runtime = PolicyExec::new(config(&workspace, NetworkPolicy::DenyAll)?)?;

    let allowed = runtime
        .run(CommandSpec::shell(format!(
            "echo hello > {}",
            guest_path(&workspace, "out/allowed.txt")
        )))
        .await?;
    assert!(allowed.success(), "{}", allowed.stderr_text());
    assert_eq!(std::fs::read_to_string(workspace.join("out/allowed.txt"))?, "hello\n");

    let denied = runtime
        .run(CommandSpec::shell(format!(
            "echo hello > {}",
            guest_path(&workspace, "out/locked/denied.txt")
        )))
        .await?;
    assert!(!denied.success());
    assert!(!workspace.join("out/locked/denied.txt").exists());
    assert!(!denied.stderr_text().is_empty());
    Ok(())
}

#[tokio::test]
async fn the_ask_callback_decides_hosts_and_denials_are_recorded() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded = Arc::clone(&asked);
    let runtime = PolicyExec::builder(config(&workspace, user_defined(vec![]))?)
        .ask_callback(move |question: AskQuestion| {
            let recorded = Arc::clone(&recorded);
            async move {
                let host = question.host.to_string();
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(host.clone());
                if host == ALLOWED_HOST {
                    AskVerdict::Allow
                } else {
                    AskVerdict::Deny(DenialReason::new(DENIAL_REASON))
                }
            }
        })
        .build()?;

    let allowed = runtime.run(curl(&format!("http://{ALLOWED_HOST}/"))).await?;
    if internet_reachable() {
        assert!(allowed.success(), "{}", allowed.stderr_text());
    }
    assert!(
        runtime
            .violations()
            .violations_for_command(&allowed.command)
            .iter()
            .all(|violation| !violation.line.as_str().contains(DENIAL_REASON))
    );

    let denied = runtime.run(curl(&format!("http://{DENIED_HOST}/"))).await?;
    assert!(!denied.success());

    let asked = asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    assert!(asked.iter().any(|host| host == ALLOWED_HOST), "{asked:?}");
    assert!(asked.iter().any(|host| host == DENIED_HOST), "{asked:?}");

    let violations = runtime.violations().violations_for_command(&denied.command);
    let denial = violations
        .iter()
        .find(|violation| violation.line.as_str().contains(DENIED_HOST))
        .ok_or_else(|| format!("no violation for {DENIED_HOST}: {violations:?}"))?;
    assert!(denial.line.as_str().contains(DENIAL_REASON));
    assert_eq!(denial.command.as_ref(), Some(&denied.command));

    let annotated = runtime.annotate_stderr(&denied.command, &denied.stderr_text());
    assert!(annotated.contains("<sandbox_violations>"), "{annotated}");
    assert!(annotated.contains(DENIED_HOST), "{annotated}");
    assert!(annotated.ends_with("</sandbox_violations>"));
    assert!(
        !runtime
            .annotate_stderr(&allowed.command, "plain")
            .contains(DENIAL_REASON)
    );
    assert_eq!(
        runtime.annotate_stderr(&trogon_sandbox_policy_exec::CommandId::new("none"), "plain"),
        "plain"
    );

    runtime.reset().await;
    assert_eq!(runtime.violations().count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_policy_allowed_range_reaches_a_local_server_until_the_policy_is_updated() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let port = serve_ok()?;
    let network = user_defined(vec![loopback()?]);
    let runtime = PolicyExec::new(config(&workspace, network)?)?;

    let before = runtime.run(curl(&format!("http://localhost:{port}/"))).await?;
    assert!(before.success(), "{}", before.stderr_text());
    assert_eq!(before.stdout_text(), "ok");

    let mut policy = runtime.policy().await;
    policy.network = NetworkPolicy::DenyAll;
    runtime.update_policy(policy).await?;

    let after = runtime.run(curl(&format!("http://localhost:{port}/"))).await?;
    assert!(!after.success());
    Ok(())
}

async fn wait_until(mut condition: impl FnMut() -> bool) -> TestResult {
    for _ in 0..300 {
        if condition() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err("the condition did not hold within 30 seconds".into())
}

fn logged_exit_codes(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_policy_update_to_deny_all_reaches_a_running_command() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let port = serve_ok()?;
    let asked = Arc::new(Mutex::new(0usize));
    let counted = Arc::clone(&asked);
    let network = user_defined(vec![loopback()?]);
    let runtime = PolicyExec::builder(config(&workspace, network)?)
        .ask_callback(move |_: AskQuestion| {
            *counted.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            async { AskVerdict::Allow }
        })
        .build()?;

    let log = workspace.join("out/log.txt");
    let script = format!(
        "while true; do curl -fsS --max-time 5 http://localhost:{port}/ >/dev/null 2>&1; echo $? >> {}; sleep 0.2; done",
        guest_path(&workspace, "out/log.txt")
    );
    let running = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.run(CommandSpec::shell(script)).await }
    });

    wait_until(|| logged_exit_codes(&log).iter().any(|code| code == "0")).await?;
    let mut policy = runtime.policy().await;
    policy.network = NetworkPolicy::DenyAll;
    runtime.update_policy(policy).await?;
    wait_until(|| logged_exit_codes(&log).last().is_some_and(|code| code != "0")).await?;
    wait_until(|| {
        runtime
            .violations()
            .violations(None)
            .iter()
            .any(|violation| violation.line.as_str().contains(&port.to_string()))
    })
    .await?;
    assert_eq!(*asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner), 0);

    runtime.reset().await;
    let output = running.await??;
    assert!(!output.success());
    Ok(())
}

#[tokio::test]
async fn output_is_capped_and_streams_can_be_discarded() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let runtime = PolicyExec::new(config(&workspace, NetworkPolicy::DenyAll)?)?;

    let capped = runtime
        .run(CommandSpec::shell("head -c 100000 /dev/zero | tr '\\0' x").output_limit(OutputLimit::Bytes(1000)))
        .await?;
    assert!(capped.success(), "{}", capped.stderr_text());
    let text = capped.stdout_text();
    assert!(text.contains("bytes truncated"), "{text}");
    assert!(text.starts_with(&"x".repeat(500)), "{text}");
    assert!(text.ends_with(&"x".repeat(500)), "{text}");
    assert!(capped.stdout.len() < 1100);
    assert!(capped.truncated.stdout);
    assert!(!capped.truncated.stderr);

    let whole = runtime
        .run(CommandSpec::shell("head -c 100000 /dev/zero | tr '\\0' x"))
        .await?;
    assert_eq!(whole.stdout.len(), 100000);
    assert!(!whole.truncated.any());

    let discarded = runtime
        .run(CommandSpec::shell("echo hidden").stdout(StdioMode::Null))
        .await?;
    assert!(discarded.success());
    assert!(discarded.stdout.is_empty());
    Ok(())
}

#[tokio::test]
async fn masked_and_denied_credentials_never_reach_the_command_in_the_clear() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let settings = trogon_sandbox_policy_exec::SandboxSettings::from_json(
        r#"{
            "network": {"allowedDomains": ["example.com"], "deniedDomains": [], "tlsTerminate": {}},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
            "credentials": {"envVars": [
                {"name": "MASKED_TOKEN", "mode": "mask"},
                {"name": "WITHHELD_TOKEN", "mode": "deny"}]}
        }"#,
    )?;
    let config = settings.into_config(&workspace)?;
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;

    let output = runtime
        .run(
            CommandSpec::shell("echo masked=$MASKED_TOKEN withheld=$WITHHELD_TOKEN")
                .env("MASKED_TOKEN", "real-masked-value")
                .env("WITHHELD_TOKEN", "real-withheld-value"),
        )
        .await?;

    assert!(output.success(), "{}", output.stderr_text());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(!stdout.contains("real-masked-value"), "{stdout}");
    assert!(!stdout.contains("real-withheld-value"), "{stdout}");
    assert!(!stdout.starts_with("masked= "), "{stdout}");
    Ok(())
}

#[tokio::test]
async fn host_view_credentials_mask_variables_and_files() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (dir, workspace) = workspace()?;
    let token_file = dir.path().join("token");
    std::fs::write(&token_file, "file-secret\n")?;
    let json = |credentials: &str| {
        format!(
            r#"{{
                "network": {{"allowedDomains": ["example.com"], "deniedDomains": [], "tlsTerminate": {{}}}},
                "filesystem": {{"denyRead": [], "allowWrite": [], "denyWrite": []}},
                "credentials": {credentials}
            }}"#
        )
    };
    let runtime = |credentials: &str| -> Result<PolicyExec, Box<dyn std::error::Error>> {
        let config = trogon_sandbox_policy_exec::SandboxSettings::from_json(&json(credentials))?
            .into_config(&workspace)?
            .view(trogon_sandbox_policy_exec::FilesystemView::Host);
        #[cfg(target_os = "linux")]
        let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
        Ok(PolicyExec::new(config)?)
    };

    let masked = runtime(
        r#"{"envVars": [
            {"name": "MASKED_TOKEN", "mode": "mask"},
            {"name": "WITHHELD_TOKEN", "mode": "deny"}]}"#,
    )?;
    let output = masked
        .run(
            CommandSpec::shell("echo masked=$MASKED_TOKEN withheld=$WITHHELD_TOKEN")
                .env("MASKED_TOKEN", "real-masked-value")
                .env("WITHHELD_TOKEN", "real-withheld-value"),
        )
        .await?;
    assert!(output.success(), "{}", output.stderr_text());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(!stdout.contains("real-masked-value"), "{stdout}");
    assert!(!stdout.contains("real-withheld-value"), "{stdout}");
    assert!(stdout.contains("masked=fake_value_"), "{stdout}");

    let file = runtime(&format!(
        r#"{{"files": [{{"path": "{}", "mode": "mask"}}]}}"#,
        token_file.display()
    ))?;
    let output = file
        .run(CommandSpec::shell(format!("cat {} 2>&1; true", token_file.display())))
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(!stdout.contains("file-secret"), "{stdout}");
    #[cfg(target_os = "linux")]
    assert!(stdout.contains("fake_value_"), "{stdout}");
    Ok(())
}

#[tokio::test]
async fn a_running_command_exposes_its_credential_state() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let settings = trogon_sandbox_policy_exec::SandboxSettings::from_json(
        r#"{
            "network": {"allowedDomains": ["example.com"], "deniedDomains": [], "tlsTerminate": {}},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
            "credentials": {"envVars": [{"name": "MASKED_TOKEN", "mode": "mask"}]}
        }"#,
    )?;
    let config = settings.into_config(&workspace)?;
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;

    let running = runtime
        .spawn(CommandSpec::shell("sleep 5").env("MASKED_TOKEN", "real-masked-value"))
        .await?;
    let credentials = runtime
        .command_credentials(running.id())
        .ok_or("the running command has no credential state")?;
    let registry = credentials.sentinel_registry().ok_or("no sentinel registry")?;
    assert_eq!(registry.len(), 1);
    let pem = credentials.ca_certificate_pem().ok_or("no ca certificate")?;
    assert!(pem.contains("BEGIN CERTIFICATE"));
    assert!(credentials.masked_files().is_empty());
    assert!(credentials.aws_pairs().is_empty());

    running.kill();
    let _ = running.wait().await;
    assert!(
        runtime
            .command_credentials(&trogon_sandbox_policy_exec::CommandId::new("unknown-command-id"))
            .is_none()
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_directory_of_the_fake_copies_stays_read_only_under_a_write_grant() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (dir, workspace) = workspace()?;
    let token_file = dir.path().join("token");
    std::fs::write(&token_file, "file-secret\n")?;
    let json = format!(
        r#"{{
            "network": {{"allowedDomains": ["example.com"], "deniedDomains": [], "tlsTerminate": {{}}}},
            "filesystem": {{"denyRead": [], "allowWrite": ["{tmp}"], "denyWrite": []}},
            "credentials": {{"files": [{{"path": "{token}", "mode": "mask"}}]}}
        }}"#,
        tmp = std::env::temp_dir().display(),
        token = token_file.display(),
    );
    let config = trogon_sandbox_policy_exec::SandboxSettings::from_json(&json)?
        .into_config(&workspace)?
        .view(trogon_sandbox_policy_exec::FilesystemView::Host)
        .helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;
    let probe = format!(
        "src=$(awk -v p={token} '$5==p {{print $4}}' /proc/self/mountinfo | head -n 1); \
         d=$(dirname \"$src\"); seen=0; [ -d \"$d\" ] && seen=1; \
         touch \"$d/planted\" 2>/dev/null && echo writable; \
         ln -s /etc/passwd \"$d/link\" 2>/dev/null && echo linkable; echo seen=$seen",
        token = token_file.display(),
    );
    let output = runtime.run(CommandSpec::shell(probe)).await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(stdout.contains("seen=1"), "{stdout}");
    assert!(!stdout.contains("writable") && !stdout.contains("linkable"), "{stdout}");
    Ok(())
}

#[tokio::test]
async fn a_terminating_command_sees_the_trust_variables_and_can_read_the_ca() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let (_dir, workspace) = workspace()?;
    let settings = trogon_sandbox_policy_exec::SandboxSettings::from_json(
        r#"{
            "network": {"allowedDomains": ["example.com"], "deniedDomains": [], "tlsTerminate": {}},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []}
        }"#,
    )?;
    let config = settings.into_config(&workspace)?;
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = PolicyExec::new(config)?;

    let output = runtime
        .run(CommandSpec::shell(
            r#"printf '%s\n' "$SSL_CERT_FILE" && head -1 "$SSL_CERT_FILE""#,
        ))
        .await?;

    assert!(output.success(), "{}", output.stderr_text());
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert!(lines[0].starts_with('/'), "{stdout}");
    assert_eq!(lines[1], "-----BEGIN CERTIFICATE-----", "{stdout}");
    Ok(())
}
