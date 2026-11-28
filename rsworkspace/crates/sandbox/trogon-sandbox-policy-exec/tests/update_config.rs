#![cfg(any(target_os = "linux", target_os = "macos"))]

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Recorded = (PolicyExec, Arc<Mutex<Vec<String>>>);

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use trogon_sandbox_contract::{AllowRange, DomainRule, DomainRuleAction};
use trogon_sandbox_contract::{DeniedDomain, DomainPattern};
use trogon_sandbox_policy_exec::{
    AskQuestion, AskVerdict, CommandId, CommandNetworkLists, CommandSpec, NetworkPolicy, PolicyExec, PolicyExecConfig,
};

fn can_sandbox() -> bool {
    let tool = |tool: &str| std::process::Command::new(tool).arg("--version").output().is_ok();
    if !PolicyExec::is_supported_platform() || !tool("curl") {
        return false;
    }
    cfg!(not(target_os = "linux")) || tool("bwrap")
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

fn config(workspace: &Path, network: NetworkPolicy) -> PolicyExecConfig {
    let config = PolicyExecConfig::new(workspace).network(network);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    config
}

fn policy(allowed: &[String], denied: &[String]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    let mut rules = Vec::new();
    for domain in allowed {
        rules.push(DomainRule {
            domain: DomainPattern::new(domain)?,
            action: DomainRuleAction::PassThrough,
        });
    }
    let mut denied_domains = Vec::new();
    for domain in denied {
        denied_domains.push(DeniedDomain::new(domain)?);
    }
    Ok(NetworkPolicy::UserDefined {
        rules,
        allow_ranges: vec![AllowRange::new("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains,
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn strict(allowed: &[String], denied: &[String]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(match policy(allowed, denied)? {
        NetworkPolicy::UserDefined {
            rules,
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            request_rules,
            ..
        } => NetworkPolicy::UserDefined {
            rules,
            allow_ranges: vec![],
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            request_rules,
        },
        other => other,
    })
}

fn target(port: u16) -> String {
    format!("localhost:{port}")
}

fn curl(port: u16) -> CommandSpec {
    CommandSpec::shell(format!("curl -s --noproxy '' --max-time 5 http://localhost:{port}/"))
}

fn curl_over_socks(port: u16) -> CommandSpec {
    CommandSpec::shell(format!(
        "curl -s --noproxy '' --max-time 5 --proxy \"$ALL_PROXY\" http://localhost:{port}/"
    ))
}

async fn reaches(runtime: &PolicyExec, port: u16) -> Result<bool, Box<dyn std::error::Error>> {
    let output = runtime.run(curl(port)).await?;
    Ok(output.stdout_text() == "ok")
}

fn lists(domain: &str) -> Result<CommandNetworkLists, Box<dyn std::error::Error>> {
    Ok(CommandNetworkLists::new(vec![DomainPattern::new(domain)?]))
}

fn id(seed: &str) -> CommandId {
    CommandId::new(format!("{seed:0<22}"))
}

fn lines(runtime: &PolicyExec) -> Vec<String> {
    runtime
        .violations()
        .violations(None)
        .iter()
        .map(|violation| violation.line.as_str().to_string())
        .collect()
}

fn recording_runtime(workspace: &Path, network: NetworkPolicy) -> Result<Recorded, Box<dyn std::error::Error>> {
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded = Arc::clone(&asked);
    let runtime = PolicyExec::builder(config(workspace, network))
        .ask_callback(move |question: AskQuestion| {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{}:{}", question.host, question.port));
            async { AskVerdict::from(false) }
        })
        .build()?;
    Ok((runtime, asked))
}

#[tokio::test]
async fn a_list_lifts_the_default_deny_for_the_command_that_carries_the_id_and_no_other() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, asked) = recording_runtime(dir.path(), strict(&[], &[])?)?;
    let registered = id("registered");
    runtime
        .register_command_network_lists(&registered, lists(&target(port))?)
        .await?;

    runtime.run(curl(port).command_id(registered.clone())).await?;
    assert!(
        asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    runtime.run(curl(port).command_id(id("other"))).await?;
    assert_eq!(
        *asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![target(port)]
    );
    runtime.run(curl(port)).await?;
    assert_eq!(
        *asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![target(port), target(port)]
    );
    Ok(())
}

#[tokio::test]
async fn a_list_applies_over_socks5_as_it_does_over_http() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, asked) = recording_runtime(dir.path(), strict(&[], &[])?)?;
    let registered = id("socks-registered");
    runtime
        .register_command_network_lists(&registered, lists(&target(port))?)
        .await?;

    runtime.run(curl_over_socks(port).command_id(registered)).await?;
    assert!(
        asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    runtime.run(curl_over_socks(port).command_id(id("socks-other"))).await?;
    assert_eq!(
        *asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![target(port)]
    );
    Ok(())
}

#[tokio::test]
async fn a_list_is_consulted_before_the_ask_callback_which_decides_every_other_host() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let (listed, unlisted) = (serve_ok()?, serve_ok()?);
    let (runtime, asked) = recording_runtime(dir.path(), strict(&[], &[])?)?;
    let registered = id("asked-registered");
    runtime
        .register_command_network_lists(&registered, lists(&target(listed))?)
        .await?;

    runtime.run(curl(listed).command_id(registered.clone())).await?;
    assert!(
        asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    runtime.run(curl(unlisted).command_id(registered)).await?;
    runtime.run(curl(listed)).await?;
    assert_eq!(
        *asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![target(unlisted), target(listed)]
    );
    Ok(())
}

#[tokio::test]
async fn a_list_follows_the_port_suffix_and_the_wildcard_of_the_allowed_domains_grammar() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, asked) = recording_runtime(dir.path(), strict(&[], &[])?)?;
    let registered = id("grammar-registered");
    let another = if port == u16::MAX { port - 1 } else { port + 1 };
    runtime
        .register_command_network_lists(
            &registered,
            CommandNetworkLists::new(vec![
                DomainPattern::new(&format!("localhost:{another}"))?,
                DomainPattern::new("*.wild.test")?,
            ]),
        )
        .await?;

    runtime.run(curl(port).command_id(registered)).await?;
    assert_eq!(
        *asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![target(port)]
    );
    Ok(())
}

#[tokio::test]
async fn a_list_never_enters_the_configured_network_policy() -> TestResult {
    let dir = tempfile::tempdir()?;
    let configured = policy(&["example.com".to_string()], &[])?;
    let runtime = PolicyExec::new(config(dir.path(), configured.clone()))?;
    runtime
        .register_command_network_lists(
            &id("never-enters"),
            CommandNetworkLists::new(vec![
                DomainPattern::new("localhost:9")?,
                DomainPattern::new("percommand.test")?,
            ]),
        )
        .await?;
    assert_eq!(runtime.policy().await.network, configured);
    assert_eq!(runtime.config().await.policy().network, configured);
    Ok(())
}

#[tokio::test]
async fn a_list_never_overrides_a_configured_denied_domain() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, asked) = recording_runtime(dir.path(), policy(&[], &["localhost".to_string()])?)?;
    let registered = id("denied-registered");
    runtime
        .register_command_network_lists(&registered, lists(&target(port))?)
        .await?;

    let output = runtime.run(curl(port).command_id(registered)).await?;
    assert_ne!(output.stdout_text(), "ok");
    assert!(
        asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    assert!(
        lines(&runtime)
            .iter()
            .any(|line| line.contains("host is on the deny list")),
        "{:?}",
        lines(&runtime)
    );
    Ok(())
}

#[tokio::test]
async fn a_policy_update_from_deny_all_opens_the_network_for_the_commands_that_follow() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let runtime = PolicyExec::new(config(dir.path(), NetworkPolicy::DenyAll))?;
    assert!(!reaches(&runtime, port).await?);

    let mut next = runtime.policy().await;
    next.network = policy(&[target(port)], &[])?;
    runtime.update_policy(next).await?;
    assert!(reaches(&runtime, port).await?);
    Ok(())
}

#[tokio::test]
async fn a_policy_update_moves_a_domain_between_the_allow_and_deny_lists() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, _asked) = recording_runtime(dir.path(), policy(&[target(port)], &[])?)?;
    assert!(reaches(&runtime, port).await?);

    for (network, reachable) in [
        (policy(&[], &[target(port)])?, false),
        (policy(&[target(port)], &[])?, true),
        (policy(&[target(port)], &[target(port)])?, false),
        (NetworkPolicy::DenyAll, false),
        (policy(&[target(port)], &[])?, true),
    ] {
        let mut next = runtime.policy().await;
        next.network = network;
        runtime.update_policy(next).await?;
        assert_eq!(reaches(&runtime, port).await?, reachable);
    }
    Ok(())
}

#[tokio::test]
async fn rapid_policy_updates_leave_the_last_policy_in_force() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let runtime = PolicyExec::new(config(dir.path(), NetworkPolicy::DenyAll))?;
    for round in 0..20 {
        let network = if round % 2 == 0 {
            policy(&[target(port)], &[])?
        } else {
            policy(&[], &[target(port)])?
        };
        let mut next = runtime.policy().await;
        next.network = network;
        runtime.update_policy(next).await?;
    }
    assert!(!reaches(&runtime, port).await?);

    let mut next = runtime.policy().await;
    next.network = policy(&[target(port)], &[])?;
    runtime.update_policy(next).await?;
    assert!(reaches(&runtime, port).await?);
    Ok(())
}

#[tokio::test]
async fn an_empty_allow_list_with_a_denied_domain_opens_once_the_update_allows_it() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let runtime = PolicyExec::new(config(dir.path(), policy(&[], &[target(port)])?))?;
    assert!(!reaches(&runtime, port).await?);

    let mut next = runtime.policy().await;
    next.network = policy(&[target(port)], &[])?;
    runtime.update_policy(next).await?;
    assert!(reaches(&runtime, port).await?);
    Ok(())
}

#[tokio::test]
async fn a_command_started_under_deny_all_follows_a_later_policy_update() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let runtime = PolicyExec::new(config(dir.path(), NetworkPolicy::DenyAll))?;
    let running = runtime
        .spawn(CommandSpec::shell(format!(
            "i=0; while [ $i -lt 100 ]; do if [ \"$(curl -s --noproxy '' --max-time 2 http://localhost:{port}/)\" = ok ]; then echo reached; exit 0; fi; i=$((i+1)); sleep 0.2; done; exit 1"
        )))
        .await
        ?;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let mut next = runtime.policy().await;
    next.network = policy(&[target(port)], &[])?;
    runtime.update_policy(next).await?;
    let exit = running.wait().await?;
    assert!(exit.status.success());
    Ok(())
}

#[tokio::test]
async fn a_command_with_an_empty_allowlist_reaches_no_host_while_the_instance_policy_allows_one() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, _asked) = recording_runtime(dir.path(), policy(&[target(port)], &[])?)?;
    assert!(reaches(&runtime, port).await?);

    let output = runtime.run(curl(port).network_policy(NetworkPolicy::DenyAll)).await?;
    assert_ne!(output.stdout_text(), "ok");
    assert!(reaches(&runtime, port).await?);
    Ok(())
}

#[tokio::test]
async fn a_denied_star_denies_every_host_whatever_the_allow_list_says() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, _asked) = recording_runtime(dir.path(), policy(&[target(port)], &["*".to_string()])?)?;
    assert!(!reaches(&runtime, port).await?);

    let mut next = runtime.policy().await;
    next.network = policy(&[target(port)], &[])?;
    runtime.update_policy(next).await?;
    assert!(reaches(&runtime, port).await?);
    Ok(())
}

#[tokio::test]
async fn a_reset_forgets_every_registered_list() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let (runtime, asked) = recording_runtime(dir.path(), strict(&[], &[])?)?;
    let registered = id("registered");
    runtime
        .register_command_network_lists(&registered, lists(&target(port))?)
        .await?;
    runtime.run(curl(port).command_id(registered.clone())).await?;
    assert!(
        asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );

    runtime.reset().await;

    runtime.run(curl(port).command_id(registered)).await?;
    assert_eq!(
        *asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![target(port)]
    );
    Ok(())
}

#[tokio::test]
async fn a_policy_update_before_any_command_has_run_is_what_the_first_command_sees() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let port = serve_ok()?;
    let runtime = PolicyExec::new(config(dir.path(), NetworkPolicy::DenyAll))?;

    let mut next = runtime.policy().await;
    next.network = policy(&[target(port)], &[])?;
    runtime.update_policy(next).await?;

    assert!(reaches(&runtime, port).await?);
    Ok(())
}
