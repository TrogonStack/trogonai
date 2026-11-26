use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use trogon_sandbox_contract::{
    LeaseEpoch, LeaseId, OwnedLease, WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId,
};
use trogon_sandbox_protocol::{AuthorityOracle, NatsAuthorityOracle, OracleError};

type TestResult = Result<(), Box<dyn Error>>;

struct Server {
    child: std::process::Child,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server_binary() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("nats-server"))
            .find(|candidate| candidate.is_file())
    });
    on_path.or_else(|| {
        let home = std::env::var_os("HOME")?;
        let shim = PathBuf::from(home).join(".local/share/mise/shims/nats-server");
        shim.is_file().then_some(shim)
    })
}

/// A private NATS server and a client, or `None` when no server can be run here.
async fn nats(options: async_nats::ConnectOptions) -> Result<Option<(Server, async_nats::Client)>, Box<dyn Error>> {
    let Some(binary) = server_binary() else {
        return Ok(None);
    };
    let dir = tempfile::tempdir()?;
    let Ok(child) = std::process::Command::new(binary)
        .args(["-a", "127.0.0.1", "-p", "-1", "--ports_file_dir"])
        .arg(dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return Ok(None);
    };
    let ports_file = dir.path().join(format!("nats-server_{}.ports", child.id()));
    let mut server = Server { child, _dir: dir };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if server.child.try_wait()?.is_some() {
            return Ok(None);
        }
        let ports = std::fs::read(&ports_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        if let Some(url) = ports.as_ref().and_then(|ports| ports["nats"][0].as_str())
            && let Ok(client) = options.clone().connect(url).await
        {
            return Ok(Some((server, client)));
        }
        if std::time::Instant::now() > deadline {
            return Err("the private NATS server did not become ready".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn request(leases: &[&str]) -> Result<WorkerAuthorityRequest, Box<dyn Error>> {
    Ok(WorkerAuthorityRequest {
        worker_id: WorkerId::new("w1")?,
        lease_ids: leases.iter().map(|id| LeaseId::new(*id)).collect(),
    })
}

const SUBJECT: &str = "sandbox.worker.authority.w1";

#[tokio::test]
async fn the_oracle_asks_the_worker_subject_and_reads_the_answer() -> TestResult {
    let Some((_server, client)) = nats(async_nats::ConnectOptions::new()).await? else {
        return Ok(());
    };
    let mut requests = client.subscribe(SUBJECT).await?;
    client.flush().await?;
    let responder = tokio::spawn({
        let client = client.clone();
        async move {
            while let Some(message) = requests.next().await {
                let Some(reply) = message.reply else { continue };
                let Ok(asked) = serde_json::from_slice::<WorkerAuthorityRequest>(&message.payload) else {
                    continue;
                };
                let answer = WorkerAuthorityResponse {
                    owned: asked
                        .lease_ids
                        .iter()
                        .map(|id| OwnedLease {
                            lease_id: id.clone(),
                            epoch: Some(LeaseEpoch::FIRST),
                        })
                        .collect(),
                    not_assigned: Vec::new(),
                };
                let Ok(body) = serde_json::to_vec(&answer) else {
                    continue;
                };
                let _ = client.publish(reply, body.into()).await;
            }
        }
    });

    let oracle = NatsAuthorityOracle::new(client, WorkerId::new("w1")?);
    let answer = oracle.owned(&request(&["a", "b"])?).await?;
    responder.abort();

    assert_eq!(
        answer.owned,
        vec![
            OwnedLease {
                lease_id: LeaseId::new("a"),
                epoch: Some(LeaseEpoch::FIRST)
            },
            OwnedLease {
                lease_id: LeaseId::new("b"),
                epoch: Some(LeaseEpoch::FIRST)
            },
        ]
    );
    assert!(answer.not_assigned.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_oracle_reports_no_responders_when_nothing_answers_the_subject() -> TestResult {
    let Some((_server, client)) = nats(async_nats::ConnectOptions::new()).await? else {
        return Ok(());
    };
    let oracle = NatsAuthorityOracle::new(client, WorkerId::new("w1")?);
    let error = oracle
        .owned(&request(&["a"])?)
        .await
        .err()
        .ok_or("a request nobody answers must fail")?;
    assert!(matches!(error, OracleError::NoResponders), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn the_oracle_reports_an_unreadable_answer() -> TestResult {
    let Some((_server, client)) = nats(async_nats::ConnectOptions::new()).await? else {
        return Ok(());
    };
    let mut requests = client.subscribe(SUBJECT).await?;
    client.flush().await?;
    let responder = tokio::spawn({
        let client = client.clone();
        async move {
            while let Some(message) = requests.next().await {
                if let Some(reply) = message.reply {
                    let _ = client.publish(reply, "not json".into()).await;
                }
            }
        }
    });
    let oracle = NatsAuthorityOracle::new(client, WorkerId::new("w1")?);
    let error = oracle
        .owned(&request(&["a"])?)
        .await
        .err()
        .ok_or("an unreadable answer must fail")?;
    responder.abort();
    assert!(matches!(error, OracleError::Decode(_)), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn the_oracle_reports_a_request_that_is_never_answered() -> TestResult {
    let options = async_nats::ConnectOptions::new().request_timeout(Some(Duration::from_millis(200)));
    let Some((_server, client)) = nats(options).await? else {
        return Ok(());
    };
    let _silent = client.subscribe(SUBJECT).await?;
    client.flush().await?;
    let oracle = NatsAuthorityOracle::new(client, WorkerId::new("w1")?);
    let error = oracle
        .owned(&request(&["a"])?)
        .await
        .err()
        .ok_or("a request nobody answers in time must fail")?;
    assert!(matches!(error, OracleError::Request(_)), "got {error:?}");
    Ok(())
}
