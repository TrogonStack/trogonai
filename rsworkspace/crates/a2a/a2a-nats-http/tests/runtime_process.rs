use std::io;
use std::net::SocketAddr;
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
    #[error("invalid startup configuration unexpectedly served HTTP")]
    UnexpectedHttpReady,
    #[error("HTTP runtime ({mode}) exited during startup: {status}; stdout: {stdout:?}; stderr: {stderr:?}")]
    ProcessExited {
        mode: RuntimeMode,
        status: ExitStatus,
        stdout: io::Result<String>,
        stderr: io::Result<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeMode {
    Direct,
    Gateway,
}

impl std::fmt::Display for RuntimeMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Direct => "direct",
            Self::Gateway => "gateway",
        })
    }
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

    async fn wait_for_http(
        &mut self,
        address: SocketAddr,
        mode: RuntimeMode,
    ) -> Result<tokio::net::TcpStream, FixtureError> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Ok(socket) = tokio::net::TcpStream::connect(address).await {
                    return Ok(socket);
                }
                if let Some(status) = self.0.try_wait()? {
                    return Err(FixtureError::ProcessExited {
                        mode,
                        status,
                        stdout: Ok(String::new()),
                        stderr: Ok(String::new()),
                    });
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?
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
async fn early_startup_exit_retains_mode_status_and_output() -> Result<(), FixtureError> {
    let address = SocketAddr::from(([127, 0, 0, 1], 0));
    for mode in [RuntimeMode::Direct, RuntimeMode::Gateway] {
        let gateway = match mode {
            RuntimeMode::Direct => "false",
            RuntimeMode::Gateway => "true",
        };
        let mut process = RuntimeProcess::start(&[
            ("A2A_AGENT_ID", "worker"),
            ("A2A_CALLER_JWT", "malformed"),
            ("A2A_USE_GATEWAY", gateway),
            ("NATS_URL", "invalid://server"),
        ])?;
        let error = process
            .wait_for_http(address, mode)
            .await
            .err()
            .ok_or(FixtureError::UnexpectedHttpReady)?;
        let FixtureError::ProcessExited {
            mode: actual_mode,
            status,
            ref stdout,
            ref stderr,
        } = error
        else {
            return Err(error);
        };
        assert_eq!(actual_mode, mode);
        assert_eq!(status.code(), Some(1));
        assert!(
            stdout
                .as_ref()
                .is_ok_and(|output| output.contains("invalid caller JWT")),
            "{error}"
        );
        assert!(stderr.as_ref().is_ok_and(String::is_empty), "{error}");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&mode.to_string()), "{diagnostic}");
        assert!(diagnostic.contains(&status.to_string()), "{diagnostic}");
        assert!(diagnostic.contains("invalid caller JWT"), "{diagnostic}");
        assert!(diagnostic.contains("stdout:"), "{diagnostic}");
        assert!(diagnostic.contains("stderr:"), "{diagnostic}");
    }
    Ok(())
}

#[tokio::test]
async fn startup_reports_invalid_configuration_before_serving() -> Result<(), FixtureError> {
    let caller = caller_jwt(fresh_expiration()?);
    for (settings, expected) in [
        (vec![], "A2A_AGENT_ID environment variable is required"),
        (vec![("A2A_PREFIX", "bad prefix")], "invalid A2A prefix"),
        (vec![("A2A_AGENT_ID", "bad agent")], "invalid agent id"),
        (
            vec![("A2A_AGENT_ID", "worker"), ("A2A_HTTP_BIND", "invalid")],
            "invalid bind address",
        ),
        (
            vec![
                ("A2A_AGENT_ID", "worker"),
                ("A2A_CALLER_JWT", caller.as_str()),
                ("NATS_URL", "invalid://server"),
            ],
            "NATS connection failed",
        ),
    ] {
        RuntimeProcess::start(&settings)?.failure(expected).await?;
    }
    Ok(())
}

fn jwt(claims: &str) -> String {
    let encode = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.{}",
        encode.encode(br#"{"alg":"HS256","typ":"JWT"}"#),
        encode.encode(claims),
        encode.encode(b"test-signature")
    )
}

fn caller_jwt(expiration: u64) -> String {
    jwt(&format!(
        r#"{{"exp":{expiration},"caller_id":"worker-caller","principal_kind":"service"}}"#
    ))
}

fn fresh_expiration() -> Result<u64, FixtureError> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + 3600)
}

#[tokio::test]
async fn startup_requires_a_fresh_caller_naming_a_principal_in_every_mode() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let expiration = fresh_expiration()?;
    let cases = [
        (None, "A2A_CALLER_JWT environment variable is required"),
        (Some("malformed".to_owned()), "invalid caller JWT"),
        (Some(caller_jwt(1)), "invalid caller JWT"),
        (
            Some(jwt(&format!(r#"{{"exp":{expiration},"caller_id":"worker-caller"}}"#))),
            "caller JWT names no caller principal",
        ),
        (
            Some(jwt(&format!(
                r#"{{"exp":{expiration},"caller_id":"worker-caller","principal_kind":"robot"}}"#
            ))),
            "caller JWT names no caller principal",
        ),
    ];
    for gateway in [false, true] {
        for (caller, expected) in &cases {
            let mut settings = vec![
                ("A2A_AGENT_ID", "worker"),
                ("NATS_URL", server.address()),
                ("NATS_TLS_DISABLED", "true"),
            ];
            if gateway {
                settings.push(("A2A_USE_GATEWAY", " On "));
            }
            if let Some(caller) = caller.as_deref() {
                settings.push(("A2A_CALLER_JWT", caller));
            }
            RuntimeProcess::start(&settings)?.failure(expected).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn startup_reports_an_occupied_http_listener() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let caller = caller_jwt(fresh_expiration()?);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    RuntimeProcess::start(&[
        ("A2A_AGENT_ID", "worker"),
        ("A2A_CALLER_JWT", &caller),
        ("NATS_URL", server.address()),
        ("NATS_TLS_DISABLED", "true"),
        ("A2A_HTTP_BIND", &address),
    ])?
    .failure("IO error")
    .await
}

#[tokio::test]
async fn serves_http_and_gracefully_stops_in_direct_and_gateway_modes() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let caller = caller_jwt(fresh_expiration()?);
    for gateway in [false, true] {
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = reservation.local_addr()?;
        let bind = address.to_string();
        drop(reservation);
        let mut settings = vec![
            ("A2A_AGENT_ID", "worker"),
            ("A2A_CALLER_JWT", &caller),
            ("NATS_URL", server.address()),
            ("NATS_TLS_DISABLED", "true"),
            ("A2A_HTTP_BIND", &bind),
        ];
        if gateway {
            settings.push(("A2A_USE_GATEWAY", "true"));
        }
        let mut process = RuntimeProcess::start(&settings)?;
        let mode = if gateway {
            RuntimeMode::Gateway
        } else {
            RuntimeMode::Direct
        };
        let mut socket = process.wait_for_http(address, mode).await?;
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
