//! Health checks for the Trogon dev stack (`trogon doctor` / `trogon --doctor`).

use agent_client_protocol::NewSessionRequest;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use trogon_registry::{AgentCapability, Registry, RegistryStore};

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

const SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(15);
const COMPACT_TIMEOUT: Duration = Duration::from_secs(10);
const DOCTOR_KV_KEY: &str = "__trogon_doctor_test__";
const COMPACT_SUBJECT: &str = "trogon.compactor.compact";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Pass,
    Fail { hint: String },
    Warn { message: String },
    Skip { reason: String },
}

#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub outcome: CheckOutcome,
    pub detail: Option<String>,
}

impl DoctorCheck {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self { name, outcome: CheckOutcome::Pass, detail: Some(detail.into()) }
    }

    fn fail(name: &'static str, hint: impl Into<String>) -> Self {
        Self {
            name,
            outcome: CheckOutcome::Fail { hint: hint.into() },
            detail: None,
        }
    }

    fn warn(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            outcome: CheckOutcome::Warn { message: message.into() },
            detail: None,
        }
    }

    fn skip(name: &'static str, reason: impl Into<String>) -> Self {
        Self {
            name,
            outcome: CheckOutcome::Skip { reason: reason.into() },
            detail: None,
        }
    }
}

/// Run all doctor checks against `nats_url` and print a colored summary.
/// Returns exit code 0 when all required checks pass (warnings are OK).
pub async fn run(nats_url: &str) -> anyhow::Result<()> {
    let checks = run_checks(nats_url).await;
    let exit_code = print_summary(&checks);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// Print doctor checks without terminating the process (REPL `/doctor`).
pub async fn print_checks(nats_url: &str) {
    let checks = run_checks(nats_url).await;
    print_summary(&checks);
}

async fn run_checks(nats_url: &str) -> Vec<DoctorCheck> {
    let mut checks = Vec::with_capacity(11);

    // 1. NATS TCP
    let nats = match async_nats::connect(nats_url).await {
        Ok(client) => {
            checks.push(DoctorCheck::pass("NATS TCP", format!("connected to {nats_url}")));
            client
        }
        Err(e) => {
            checks.push(DoctorCheck::fail(
                "NATS TCP",
                format!("Start `nats-server -p 4222 -js` ({e})"),
            ));
            checks.extend(remaining_skipped_checks());
            return checks;
        }
    };

    // 2. JetStream KV
    checks.push(check_jetstream_kv(&nats).await);

    // Registry (checks 3–7)
    let js = async_nats::jetstream::new(nats.clone());
    let registry = match trogon_registry::provision(&js).await {
        Ok(store) => Registry::new(store),
        Err(e) => {
            checks.push(DoctorCheck::fail(
                "Registry",
                format!("NATS missing `-js` or registry unavailable ({e})"),
            ));
            checks.extend(registry_dependent_skipped());
            checks.push(check_session_smoke(&nats).await);
            checks.push(check_compactor(&nats).await);
            checks.push(check_codex_cli());
            checks.push(check_token_warnings());
            return checks;
        }
    };

    checks.push(check_registry_prefix(&registry, "acp.claude", "ANTHROPIC_TOKEN", "trogon-acp-runner").await);
    checks.push(check_registry_prefix(&registry, "acp.grok", "XAI_API_KEY", "trogon-xai-runner").await);
    checks.push(
        check_registry_prefix(&registry, "acp.openrouter", "OPENROUTER_API_KEY", "trogon-openrouter-runner")
            .await,
    );
    checks.push(check_registry_codex(&registry).await);
    checks.push(check_registry_execution(&registry).await);

    // 8. Session smoke
    checks.push(check_session_smoke(&nats).await);

    // 9. Compactor
    checks.push(check_compactor(&nats).await);

    // 10. Codex CLI
    checks.push(check_codex_cli());

    // 11. Token warnings
    checks.push(check_token_warnings());

    checks
}

fn remaining_skipped_checks() -> Vec<DoctorCheck> {
    vec![
        DoctorCheck::skip("JetStream KV", "NATS unreachable"),
        DoctorCheck::skip("Registry acp.claude", "NATS unreachable"),
        DoctorCheck::skip("Registry acp.grok", "NATS unreachable"),
        DoctorCheck::skip("Registry acp.openrouter", "NATS unreachable"),
        DoctorCheck::skip("Registry acp.codex", "NATS unreachable"),
        DoctorCheck::skip("Registry execution", "NATS unreachable"),
        DoctorCheck::skip("Session smoke", "NATS unreachable"),
        DoctorCheck::skip("Compactor", "NATS unreachable"),
        DoctorCheck::skip("Codex CLI", "NATS unreachable"),
        DoctorCheck::skip("Token warnings", "NATS unreachable"),
    ]
}

fn registry_dependent_skipped() -> Vec<DoctorCheck> {
    vec![
        DoctorCheck::skip("Registry acp.claude", "registry unavailable"),
        DoctorCheck::skip("Registry acp.grok", "registry unavailable"),
        DoctorCheck::skip("Registry acp.openrouter", "registry unavailable"),
        DoctorCheck::skip("Registry acp.codex", "registry unavailable"),
        DoctorCheck::skip("Registry execution", "registry unavailable"),
    ]
}

async fn check_jetstream_kv(nats: &async_nats::Client) -> DoctorCheck {
    let js = async_nats::jetstream::new(nats.clone());
    let store = match js.create_key_value(temp_kv_config()).await {
        Ok(store) => store,
        Err(_) => match js.get_key_value("TROGON_DOCTOR_TEST").await {
            Ok(store) => store,
            Err(e) => return DoctorCheck::fail("JetStream KV", format!("NATS missing `-js` ({e})")),
        },
    };

    let put = store.put(DOCTOR_KV_KEY, "ok".into()).await;
    let get = store.get(DOCTOR_KV_KEY).await;
    match (put, get) {
        (Ok(_), Ok(Some(entry))) if entry.as_ref() == b"ok" => {
            let _ = store.delete(DOCTOR_KV_KEY).await;
            DoctorCheck::pass("JetStream KV", "put/get OK")
        }
        (Ok(_), Ok(_)) => DoctorCheck::fail("JetStream KV", "KV get returned unexpected value"),
        (Err(e), _) => DoctorCheck::fail("JetStream KV", format!("NATS missing `-js` ({e})")),
        (_, Err(e)) => DoctorCheck::fail("JetStream KV", format!("NATS missing `-js` ({e})")),
    }
}

fn temp_kv_config() -> async_nats::jetstream::kv::Config {
    async_nats::jetstream::kv::Config {
        bucket: "TROGON_DOCTOR_TEST".to_string(),
        history: 1,
        ..Default::default()
    }
}

async fn check_registry_prefix<S: RegistryStore>(
    registry: &Registry<S>,
    acp_prefix: &str,
    env_var: &str,
    runner_bin: &str,
) -> DoctorCheck {
    let name = format!("Registry {acp_prefix}");
    if !env_var_nonempty(env_var) {
        return DoctorCheck::skip(leak_name(name), format!("{env_var} not set"));
    }
    match find_by_acp_prefix(registry, acp_prefix).await {
        Ok(Some(_)) => DoctorCheck::pass(leak_name(name), "registered"),
        Ok(None) => DoctorCheck::fail(
            leak_name(name),
            format!("Start `{runner_bin}`"),
        ),
        Err(e) => DoctorCheck::fail(leak_name(name), e),
    }
}

async fn check_registry_codex<S: RegistryStore>(registry: &Registry<S>) -> DoctorCheck {
    if !codex_enabled() {
        return DoctorCheck::skip("Registry acp.codex", "CODEX_ENABLED != 1");
    }
    match find_by_acp_prefix(registry, "acp.codex").await {
        Ok(Some(_)) => DoctorCheck::pass("Registry acp.codex", "registered"),
        Ok(None) => DoctorCheck::fail("Registry acp.codex", "Start `trogon-codex-runner`"),
        Err(e) => DoctorCheck::fail("Registry acp.codex", e),
    }
}

async fn check_registry_execution<S: RegistryStore>(registry: &Registry<S>) -> DoctorCheck {
    match registry.discover("execution").await {
        Ok(agents) if agents.iter().any(|a| a.has_capability("execution")) => {
            DoctorCheck::pass("Registry execution", "wasm-runtime registered")
        }
        Ok(_) => DoctorCheck::fail(
            "Registry execution",
            "Start `trogon-wasm-runtime` with `WASM_ONLY=0`",
        ),
        Err(e) => DoctorCheck::fail("Registry execution", e.to_string()),
    }
}

async fn check_session_smoke(nats: &async_nats::Client) -> DoctorCheck {
    let prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp.claude".into());
    let subject = format!("{prefix}.agent.session.new");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));
    let req = NewSessionRequest::new(cwd);
    let payload = match serde_json::to_vec(&req) {
        Ok(p) => p,
        Err(e) => return DoctorCheck::fail("Session smoke", e.to_string()),
    };

    match tokio::time::timeout(SESSION_NEW_TIMEOUT, nats.request(subject, payload.into())).await {
        Ok(Ok(response)) => {
            match serde_json::from_slice::<serde_json::Value>(&response.payload) {
                Ok(v) if v.get("sessionId").and_then(|s| s.as_str()).is_some() => {
                    DoctorCheck::pass("Session smoke", format!("{prefix} session.new OK"))
                }
                Ok(v) => DoctorCheck::fail(
                    "Session smoke",
                    format!("Runner down or wrong prefix (bad response: {v})"),
                ),
                Err(e) => DoctorCheck::fail("Session smoke", format!("Invalid response: {e}")),
            }
        }
        Ok(Err(e)) => DoctorCheck::fail(
            "Session smoke",
            format!("Runner down or wrong prefix ({e})"),
        ),
        Err(_) => DoctorCheck::fail(
            "Session smoke",
            format!("Timed out after {}s — is `{prefix}` runner running?", SESSION_NEW_TIMEOUT.as_secs()),
        ),
    }
}

async fn check_compactor(nats: &async_nats::Client) -> DoctorCheck {
    let payload = serde_json::to_vec(&json!({ "messages": [] })).unwrap();
    match tokio::time::timeout(COMPACT_TIMEOUT, nats.request(COMPACT_SUBJECT, payload.into())).await
    {
        Ok(Ok(_response)) => DoctorCheck::pass("Compactor", "trogon.compactor.compact OK"),
        Ok(Err(e)) => DoctorCheck::fail("Compactor", format!("Start `trogon-compactor` ({e})")),
        Err(_) => DoctorCheck::fail(
            "Compactor",
            format!("Start `trogon-compactor` (no response within {}s)", COMPACT_TIMEOUT.as_secs()),
        ),
    }
}

fn check_codex_cli() -> DoctorCheck {
    if !codex_enabled() {
        return DoctorCheck::skip("Codex CLI", "CODEX_ENABLED != 1");
    }
    match Command::new("codex").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            DoctorCheck::pass("Codex CLI", if version.is_empty() { "codex --version OK".into() } else { version })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            DoctorCheck::fail("Codex CLI", format!("Install Codex CLI ({stderr})"))
        }
        Err(e) => DoctorCheck::fail("Codex CLI", format!("Install Codex CLI ({e})")),
    }
}

fn check_token_warnings() -> DoctorCheck {
    let warnings = collect_token_warnings();
    if warnings.is_empty() {
        DoctorCheck::pass("Token warnings", "all configured keys non-empty")
    } else {
        DoctorCheck::warn("Token warnings", warnings.join("; "))
    }
}

/// Warn when the active runner or an explicitly configured runner has an empty API key.
pub fn collect_token_warnings() -> Vec<String> {
    let mut warnings = Vec::new();
    let prefix = std::env::var("ACP_PREFIX").unwrap_or_else(|_| "acp.claude".into());

    let active = match prefix.as_str() {
        "acp.claude" => Some(("ANTHROPIC_TOKEN", "Claude")),
        "acp.grok" => Some(("XAI_API_KEY", "Grok")),
        "acp.openrouter" => Some(("OPENROUTER_API_KEY", "OpenRouter")),
        "acp.codex" => None,
        _ => None,
    };
    if let Some((var, label)) = active {
        if !env_var_nonempty(var) {
            warnings.push(format!("{var} is empty for active runner ({label}) — fill `.env.local`"));
        }
    }

    for (var, label) in [
        ("ANTHROPIC_TOKEN", "Claude"),
        ("XAI_API_KEY", "Grok"),
        ("OPENROUTER_API_KEY", "OpenRouter"),
    ] {
        if std::env::var(var).is_ok() && !env_var_nonempty(var) {
            warnings.push(format!("{var} is set but empty ({label})"));
        }
    }

    warnings
}

pub async fn find_by_acp_prefix<S: RegistryStore>(
    registry: &Registry<S>,
    acp_prefix: &str,
) -> Result<Option<AgentCapability>, String> {
    let all = registry.list_all().await.map_err(|e| e.to_string())?;
    Ok(all.into_iter().find(|cap| {
        cap.metadata
            .get("acp_prefix")
            .and_then(|v| v.as_str())
            .is_some_and(|p| p == acp_prefix)
    }))
}

fn env_var_nonempty(key: &str) -> bool {
    std::env::var(key).map(|v| !v.trim().is_empty()).unwrap_or(false)
}

fn codex_enabled() -> bool {
    std::env::var("CODEX_ENABLED").map(|v| v == "1").unwrap_or(false)
}

/// Leak a formatted name so it can be used as `&'static str` in check labels.
fn leak_name(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

fn print_summary(checks: &[DoctorCheck]) -> i32 {
    eprintln!();
    eprintln!("{BOLD}Trogon Doctor{RESET}");
    eprintln!("{DIM}{}{RESET}", "─".repeat(40));

    let mut failures = 0u32;
    for check in checks {
        let (icon, color) = match &check.outcome {
            CheckOutcome::Pass => ("✓", GREEN),
            CheckOutcome::Fail { .. } => {
                failures += 1;
                ("✗", RED)
            }
            CheckOutcome::Warn { .. } => ("!", YELLOW),
            CheckOutcome::Skip { .. } => ("−", DIM),
        };
        eprint!("{color}{icon}{RESET} {:<22}", check.name);
        match &check.outcome {
            CheckOutcome::Pass => {
                if let Some(detail) = &check.detail {
                    eprintln!("{detail}");
                } else {
                    eprintln!();
                }
            }
            CheckOutcome::Fail { hint } => eprintln!("{RED}{hint}{RESET}"),
            CheckOutcome::Warn { message } => eprintln!("{YELLOW}{message}{RESET}"),
            CheckOutcome::Skip { reason } => eprintln!("{DIM}{reason}{RESET}"),
        }
    }

    eprintln!();
    let passed = checks.iter().filter(|c| matches!(c.outcome, CheckOutcome::Pass)).count();
    let warned = checks.iter().filter(|c| matches!(c.outcome, CheckOutcome::Warn { .. })).count();
    let skipped = checks.iter().filter(|c| matches!(c.outcome, CheckOutcome::Skip { .. })).count();
    eprintln!(
        "{BOLD}{passed}{RESET} passed, {BOLD}{failures}{RESET} failed, {warned} warnings, {skipped} skipped"
    );

    if failures > 0 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use trogon_registry::{MockRegistryStore, Registry};

    /// Token-related env vars read by [`collect_token_warnings`] / [`codex_enabled`].
    const TOKEN_ENV_KEYS: &[&str] = &[
        "ACP_PREFIX",
        "ANTHROPIC_TOKEN",
        "XAI_API_KEY",
        "OPENROUTER_API_KEY",
        "CODEX_ENABLED",
    ];

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Serializes env mutation across doctor unit tests and restores prior values on drop.
    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = ENV_TEST_LOCK.lock().unwrap();
            let saved = TOKEN_ENV_KEYS
                .iter()
                .map(|k| ((*k).to_string(), std::env::var(k).ok()))
                .collect();
            Self { saved, _lock: lock }
        }

        fn clear_token_env(&self) {
            for key in TOKEN_ENV_KEYS {
                unsafe { std::env::remove_var(key) };
            }
        }

        fn set(&self, key: &str, value: &str) {
            unsafe { std::env::set_var(key, value) };
        }

        fn remove(&self, key: &str) {
            unsafe { std::env::remove_var(key) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(v) => unsafe { std::env::set_var(key, v) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    #[test]
    fn collect_token_warnings_empty_when_keys_set() {
        let guard = EnvGuard::new();
        guard.clear_token_env();
        guard.set("ACP_PREFIX", "acp.claude");
        guard.set("ANTHROPIC_TOKEN", "sk-test");
        guard.set("CODEX_ENABLED", "0");
        let warnings = collect_token_warnings();
        assert!(warnings.is_empty(), "expected no warnings, got: {warnings:?}");
    }

    #[test]
    fn collect_token_warnings_active_runner_empty_token() {
        let guard = EnvGuard::new();
        guard.clear_token_env();
        guard.set("ACP_PREFIX", "acp.grok");
        let warnings = collect_token_warnings();
        assert!(
            warnings.iter().any(|w| w.contains("XAI_API_KEY")),
            "expected XAI warning, got: {warnings:?}"
        );
    }

    #[test]
    fn collect_token_warnings_set_but_empty() {
        let guard = EnvGuard::new();
        guard.clear_token_env();
        guard.set("OPENROUTER_API_KEY", "   ");
        let warnings = collect_token_warnings();
        assert!(
            warnings.iter().any(|w| w.contains("OPENROUTER_API_KEY")),
            "expected empty-key warning, got: {warnings:?}"
        );
    }

    #[tokio::test]
    async fn find_by_acp_prefix_matches_metadata() {
        let r = Registry::new(MockRegistryStore::new());
        let mut cap = AgentCapability::new("claude", ["chat"], "agents.claude.>");
        cap.metadata = serde_json::json!({ "acp_prefix": "acp.claude" });
        r.register(&cap).await.unwrap();

        let found = find_by_acp_prefix(&r, "acp.claude").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().agent_type, "claude");
    }

    #[tokio::test]
    async fn find_by_acp_prefix_returns_none_when_missing() {
        let r = Registry::new(MockRegistryStore::new());
        let found = find_by_acp_prefix(&r, "acp.grok").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn registry_execution_discover_finds_wasm() {
        let r = Registry::new(MockRegistryStore::new());
        let mut cap = AgentCapability::new("wasm", ["execution"], "acp.wasm.agent.>");
        cap.metadata = serde_json::json!({ "acp_prefix": "acp.wasm" });
        r.register(&cap).await.unwrap();

        let agents = r.discover("execution").await.unwrap();
        assert_eq!(agents.len(), 1);
        assert!(agents[0].has_capability("execution"));
    }

    #[test]
    fn codex_enabled_reads_env() {
        let guard = EnvGuard::new();
        guard.clear_token_env();
        guard.set("CODEX_ENABLED", "1");
        assert!(codex_enabled());
        guard.set("CODEX_ENABLED", "0");
        assert!(!codex_enabled());
        guard.remove("CODEX_ENABLED");
        assert!(!codex_enabled());
    }

    #[test]
    fn print_summary_counts_failures() {
        let checks = vec![
            DoctorCheck::pass("A", "ok"),
            DoctorCheck::fail("B", "hint"),
        ];
        assert_eq!(print_summary(&checks), 1);
    }

    #[tokio::test]
    #[ignore = "requires NATS with JetStream at NATS_URL"]
    async fn doctor_integration_all_checks() {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
        let checks = run_checks(&url).await;
        assert!(
            checks.iter().any(|c| c.name == "NATS TCP"),
            "expected NATS TCP check"
        );
    }
}
