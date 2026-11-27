#![cfg(feature = "server")]

use std::error::Error;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn Error>>;

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-actor-agent");

fn free_port() -> Result<u16, Box<dyn Error>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn agent(port: u16, extra: &[&str]) -> Result<Child, Box<dyn Error>> {
    Ok(Command::new(BINARY)
        .args(["--bind", "127.0.0.1", "--port", &port.to_string()])
        .args(extra)
        .env_remove("SANDBOX_ACTOR_AGENT_VSOCK_PORT")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?)
}

fn interrupt(child: &Child) -> TestResult {
    let status = Command::new("kill").args(["-INT", &child.id().to_string()]).status()?;
    assert!(status.success());
    Ok(())
}

fn finished(child: &mut Child, within: Duration) -> Result<Option<std::process::ExitStatus>, Box<dyn Error>> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(None)
}

#[tokio::test]
async fn the_binary_serves_until_interrupted_and_then_exits_cleanly() -> TestResult {
    let port = free_port()?;
    let mut child = agent(port, &[])?;
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/readyz");
    let mut ready = false;
    for _ in 0..500 {
        if client
            .get(&url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if !ready {
        child.kill()?;
        return Err("the agent never became ready".into());
    }
    interrupt(&child)?;
    let status = finished(&mut child, Duration::from_secs(20))?;
    if status.is_none() {
        child.kill()?;
    }
    assert!(status.is_some_and(|status| status.success()), "got {status:?}");
    Ok(())
}

#[test]
fn a_port_already_taken_is_a_startup_failure() -> TestResult {
    let taken = std::net::TcpListener::bind("127.0.0.1:0")?;
    let mut child = agent(taken.local_addr()?.port(), &[])?;
    let status = finished(&mut child, Duration::from_secs(20))?;
    if status.is_none() {
        child.kill()?;
    }
    assert!(status.is_some_and(|status| !status.success()), "got {status:?}");
    Ok(())
}

#[test]
fn a_vsock_port_either_serves_or_fails_to_start() -> TestResult {
    let mut child = agent(free_port()?, &["--vsock-port", "4294967294"])?;
    let status = finished(&mut child, Duration::from_secs(2))?;
    match status {
        Some(status) => assert!(!status.success()),
        None => {
            interrupt(&child)?;
            finished(&mut child, Duration::from_secs(20))?;
            child.kill()?;
        }
    }
    Ok(())
}
