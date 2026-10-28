#![cfg(feature = "spicedb")]

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::{EnumerateEnv, SystemEnv};

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error(transparent)]
    Connect(#[from] async_nats::ConnectError),
    #[error(transparent)]
    Nkey(#[from] nkeys::error::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("gateway exited during startup: {0}")]
    ProcessExited(ExitStatus),
}

struct GatewayProcess(Child);

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[tokio::test]
async fn gateway_subscribes_dispatches_and_stops_for_both_subscription_modes() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let signing_key = nkeys::KeyPair::new_account().seed()?;
    for queue in [None, Some("runtime-workers")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-gateway"));
        command
            .env_clear()
            .envs(SystemEnv.vars_os().into_iter().filter(|(key, _)| {
                matches!(
                    key.to_str(),
                    Some("LLVM_PROFILE_FILE" | "LD_LIBRARY_PATH" | "DYLD_FALLBACK_LIBRARY_PATH")
                )
            }))
            .env("AUTH_CALLOUT_SIGNING_SECRET", &signing_key)
            .args(["--nats-url", server.address(), "--prefix", "a2a"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(queue) = queue {
            command.args(["--queue-group", queue]);
        }
        let mut process = GatewayProcess(command.spawn()?);
        let reply = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let reply = client
                    .request(
                        "a2a.v1.gateway.worker.unknown",
                        br#"{"jsonrpc":"2.0","id":"startup","method":"unknown"}"#.as_slice().into(),
                    )
                    .await;
                if let Ok(reply) = reply {
                    return Ok::<_, FixtureError>(reply);
                }
                if let Some(status) = process.0.try_wait()? {
                    return Err(FixtureError::ProcessExited(status));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        let body: serde_json::Value = serde_json::from_slice(&reply.payload)?;
        assert_eq!(body["id"], "startup");
        assert_eq!(body["error"]["code"], -32600);
        assert!(
            Command::new("/bin/kill")
                .arg("-TERM")
                .arg(process.0.id().to_string())
                .status()?
                .success()
        );
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = process.0.try_wait()? {
                    return Ok::<_, FixtureError>(status);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        assert!(status.success(), "gateway must drain and exit successfully: {status}");
    }
    Ok(())
}
