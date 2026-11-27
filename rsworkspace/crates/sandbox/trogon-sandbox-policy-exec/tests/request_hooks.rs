#![cfg(any(target_os = "linux", target_os = "macos"))]

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Recording = (u16, Arc<Mutex<Vec<String>>>);

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use trogon_sandbox_policy_exec::{
    AllowRange, CommandSpec, DeciderLimits, DeciderTransport, DeciderTransportError, NetworkPolicy, PolicyExec,
    PolicyExecConfig,
};

fn can_sandbox() -> bool {
    let tool = |name: &str| std::process::Command::new(name).arg("--version").output().is_ok();
    PolicyExec::is_supported_platform() && tool("curl") && (cfg!(not(target_os = "linux")) || tool("bwrap"))
}

fn loopback_policy() -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("127.0.0.0/8")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn config(workspace: &Path) -> Result<PolicyExecConfig, Box<dyn std::error::Error>> {
    let config = PolicyExecConfig::new(workspace).network(loopback_policy()?);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    Ok(config)
}

fn serve_recording() -> Result<Recording, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buffer = [0u8; 4096];
            let n = stream.read(&mut buffer).unwrap_or(0);
            if n == 0 {
                continue;
            }
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(String::from_utf8_lossy(&buffer[..n]).into_owned());
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    Ok((port, seen))
}

struct BodyDecider {
    asked: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl DeciderTransport for BodyDecider {
    async fn request(&self, _subject: String, payload: Vec<u8>) -> Result<Vec<u8>, DeciderTransportError> {
        let question: serde_json::Value =
            serde_json::from_slice(&payload).map_err(|error| DeciderTransportError::Unavailable(error.to_string()))?;
        let reply = match question.get("body") {
            None => r#"{"verdict":"need_body","max":16}"#,
            Some(body) if body["data"] == "aGVsbG8=" => {
                r#"{"verdict":"allow","set_headers":{"x-seen-by":["decider"]}}"#
            }
            Some(_) => r#"{"verdict":"deny","status":403}"#,
        };
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(question);
        Ok(reply.as_bytes().to_vec())
    }
}

#[tokio::test]
async fn the_plaintext_header_mutator_edits_what_the_command_sends() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let workspace = dir.path().canonicalize()?;
    let (port, seen) = serve_recording()?;
    let runtime = PolicyExec::builder(config(&workspace)?)
        .header_mutator_plaintext(|headers: &mut Vec<(String, String)>, _: &_| {
            headers.push(("X-Mutated".to_string(), "1".to_string()));
        })
        .build()?;

    let out = runtime
        .run(CommandSpec::shell(format!(
            "curl -fsS --max-time 20 http://localhost:{port}/"
        )))
        .await?;
    assert!(out.success(), "{}", out.stderr_text());
    let request = seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[0].to_ascii_lowercase();
    assert!(request.contains("x-mutated: 1\r\n"), "{request}");
    Ok(())
}

#[tokio::test]
async fn the_request_decider_gets_the_plain_http_body_it_asked_for() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let workspace = dir.path().canonicalize()?;
    let (port, seen) = serve_recording()?;
    let decider = Arc::new(BodyDecider {
        asked: Mutex::new(Vec::new()),
    });
    let runtime = PolicyExec::builder(config(&workspace)?)
        .plaintext_header_set(true)
        .request_decider(decider.clone(), DeciderLimits::default(), Vec::new())
        .build()?;

    let post = |data: &str| CommandSpec::shell(format!("curl -fsS --max-time 20 -d {data} http://localhost:{port}/"));
    let allowed = runtime.run(post("hello")).await?;
    assert!(
        allowed.success(),
        "{} {:?}",
        allowed.stderr_text(),
        decider.asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    );
    let denied = runtime.run(post("other")).await?;
    assert!(!denied.success());

    let request = seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[0].to_ascii_lowercase();
    assert!(request.contains("x-seen-by: decider\r\n"), "{request}");
    assert_eq!(seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len(), 1);
    assert_eq!(
        decider
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        4
    );
    Ok(())
}
