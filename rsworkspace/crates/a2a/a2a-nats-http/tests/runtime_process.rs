use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::{EnumerateEnv, SystemEnv};

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error(transparent)]
    SystemTime(#[from] std::time::SystemTimeError),
    #[error("HTTP runtime exited during startup: {0}")]
    ProcessExited(ExitStatus),
}

struct RuntimeProcess(Child);

impl RuntimeProcess {
    fn start(settings: &[(&str, &str)]) -> io::Result<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-nats-http"));
        command
            .env_clear()
            .envs(SystemEnv.vars_os().into_iter().filter(|(key, _)| {
                matches!(
                    key.to_str(),
                    Some("LLVM_PROFILE_FILE" | "LD_LIBRARY_PATH" | "DYLD_FALLBACK_LIBRARY_PATH")
                )
            }))
            .env("A2A_CONNECT_TIMEOUT_SECS", "1")
            .envs(settings.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().map(Self)
    }

    async fn wait(&mut self) -> Result<ExitStatus, FixtureError> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(status) = self.0.try_wait()? {
                    return Ok::<_, FixtureError>(status);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?
    }

    async fn failure(&mut self, expected: &str) -> Result<(), FixtureError> {
        assert_eq!(self.wait().await?.code(), Some(1));
        let mut output = String::new();
        if let Some(mut stdout) = self.0.stdout.take() {
            std::io::Read::read_to_string(&mut stdout, &mut output)?;
        }
        if let Some(mut stderr) = self.0.stderr.take() {
            std::io::Read::read_to_string(&mut stderr, &mut output)?;
        }
        assert!(output.contains(expected), "missing {expected:?} in {output}");
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), FixtureError> {
        assert!(
            Command::new("/bin/kill")
                .arg("-TERM")
                .arg(self.0.id().to_string())
                .status()?
                .success()
        );
        assert!(self.wait().await?.success());
        Ok(())
    }
}

impl Drop for RuntimeProcess {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[tokio::test]
async fn startup_reports_invalid_configuration_before_serving() -> Result<(), FixtureError> {
    for (settings, expected) in [
        (vec![], "A2A_AGENT_ID environment variable is required"),
        (vec![("A2A_PREFIX", "bad prefix")], "invalid A2A prefix"),
        (vec![("A2A_AGENT_ID", "bad agent")], "invalid agent id"),
        (
            vec![("A2A_AGENT_ID", "worker"), ("A2A_HTTP_BIND", "invalid")],
            "invalid bind address",
        ),
        (
            vec![("A2A_AGENT_ID", "worker"), ("NATS_URL", "invalid://server")],
            "NATS connection failed",
        ),
    ] {
        RuntimeProcess::start(&settings)?.failure(expected).await?;
    }
    Ok(())
}

fn caller_jwt(expiration: u64) -> String {
    let encode = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.{}",
        encode.encode(br#"{"alg":"HS256","typ":"JWT"}"#),
        encode.encode(format!(r#"{{"exp":{expiration}}}"#)),
        encode.encode(b"test-signature")
    )
}

#[tokio::test]
async fn gateway_routing_requires_a_well_formed_unexpired_caller() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    for caller in [None, Some("malformed".to_owned()), Some(caller_jwt(1))] {
        let mut settings = vec![
            ("A2A_AGENT_ID", "worker"),
            ("NATS_URL", server.address()),
            ("A2A_USE_GATEWAY", " On "),
        ];
        if let Some(caller) = caller.as_deref() {
            settings.push(("A2A_GATEWAY_CALLER_JWT", caller));
        }
        RuntimeProcess::start(&settings)?
            .failure(if caller.is_none() {
                "A2A_GATEWAY_CALLER_JWT is required"
            } else {
                "invalid gateway caller JWT"
            })
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn startup_reports_an_occupied_http_listener() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    RuntimeProcess::start(&[
        ("A2A_AGENT_ID", "worker"),
        ("NATS_URL", server.address()),
        ("A2A_HTTP_BIND", &address),
    ])?
    .failure("IO error")
    .await
}

#[tokio::test]
async fn serves_http_and_gracefully_stops_in_direct_and_gateway_modes() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let expiration = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 3600;
    let caller = caller_jwt(expiration);
    for gateway in [false, true] {
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = reservation.local_addr()?;
        let bind = address.to_string();
        drop(reservation);
        let mut settings = vec![
            ("A2A_AGENT_ID", "worker"),
            ("NATS_URL", server.address()),
            ("A2A_HTTP_BIND", &bind),
        ];
        if gateway {
            settings.extend([("A2A_USE_GATEWAY", "true"), ("A2A_GATEWAY_CALLER_JWT", &caller)]);
        }
        let mut process = RuntimeProcess::start(&settings)?;
        let mut socket = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Ok(socket) = tokio::net::TcpStream::connect(address).await {
                    return Ok::<_, FixtureError>(socket);
                }
                if let Some(status) = process.0.try_wait()? {
                    return Err(FixtureError::ProcessExited(status));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        socket
            .write_all(b"GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(5), socket.read_to_string(&mut response)).await??;
        assert!(response.starts_with("HTTP/1.1 404"), "{response}");
        process.stop().await?;
    }
    Ok(())
}
