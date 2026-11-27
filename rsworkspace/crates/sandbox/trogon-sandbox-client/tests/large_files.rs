//! Transfers larger than the NATS server's maximum payload, against a real server.

use std::time::Duration;

use async_nats::Client;
use futures::StreamExt;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{LeaseId, ReadFileResponse, SandboxFileWrite, WriteFilesRequest, WriteFilesResponse};
use trogon_sandbox_protocol::nats_transfer;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DEADLINE: Duration = Duration::from_secs(60);

async fn start_small_payload_server() -> Result<(CoreTestServer, Client), async_nats::ConnectError> {
    let server = CoreTestServer::start_with_config("max_payload: 65536\n").await;
    let client = async_nats::connect(format!("nats://{}", server.address())).await?;
    Ok((server, client))
}

#[tokio::test]
async fn write_file_larger_than_nats_payload_limit() -> TestResult {
    tokio::time::timeout(DEADLINE, async {
        let (_server, nats) = start_small_payload_server().await?;
        let mut broker_requests = nats.subscribe("sandbox.lease.*.*").await?;
        let mut worker_requests = nats.subscribe("relay.*.*").await?;
        nats.flush().await?;
        let expected: Vec<u8> = (0..512 * 1024).map(|i| (i % 256) as u8).collect();
        let expected_worker = expected.clone();
        let worker = nats.clone();
        let handler = tokio::spawn(async move {
            for _ in 0..2 {
                let message = worker_requests.next().await.ok_or("worker subscription closed")?;
                let payload = nats_transfer::receive(&worker, &message).await?;
                let response = if message.subject.contains(".write_files.") {
                    let request: WriteFilesRequest = serde_json::from_slice(&payload)?;
                    assert_eq!(request.files[0].content, expected_worker);
                    assert_eq!(request.files[0].mode, Some(0o600));
                    serde_json::to_vec(&WriteFilesResponse::Written)?
                } else {
                    serde_json::to_vec(&ReadFileResponse::Content {
                        data: expected_worker.clone(),
                    })?
                };
                let reply = message.reply.as_ref().ok_or("request without a reply subject")?;
                nats_transfer::respond(&worker, reply.as_str(), response.into()).await?;
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
        let broker = nats.clone();
        let relay = tokio::spawn(async move {
            for _ in 0..2 {
                let message = broker_requests.next().await.ok_or("broker subscription closed")?;
                let payload = nats_transfer::receive(&broker, &message).await?;
                let subject = message.subject.as_str().replace("sandbox.lease.", "relay.");
                let response =
                    nats_transfer::request(&broker, &subject, payload, None, Duration::from_secs(10)).await?;
                let reply = message.reply.as_ref().ok_or("request without a reply subject")?;
                nats_transfer::respond(&broker, reply.as_str(), response).await?;
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
        let client = SandboxClient::from_client(nats);
        let lease_id = LeaseId::new("large-file");
        client
            .write_files(
                &lease_id,
                vec![SandboxFileWrite {
                    path: "/tmp/data".into(),
                    content: expected.clone(),
                    mode: Some(0o600),
                }],
                None,
            )
            .await?;
        assert_eq!(client.read_file(&lease_id, "/tmp/data".into(), None).await?, expected);
        handler.await?.map_err(|error| error.to_string())?;
        relay.await?.map_err(|error| error.to_string())?;
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn cancelling_a_large_request_removes_its_chunk_server() -> TestResult {
    tokio::time::timeout(DEADLINE, async {
        let (_server, nats) = start_small_payload_server().await?;
        let mut requests = nats.subscribe("test.transfer").await?;
        nats.flush().await?;
        let sender = nats.clone();
        let pending = tokio::spawn(async move {
            nats_transfer::request(
                &sender,
                "test.transfer",
                vec![255; 1024 * 1024].into(),
                None,
                Duration::from_secs(10),
            )
            .await
        });
        let descriptor = requests.next().await.ok_or("subscription closed")?;
        let transfer: serde_json::Value = serde_json::from_slice(&descriptor.payload)?;
        let inbox = transfer["inbox"]
            .as_str()
            .ok_or("descriptor without an inbox")?
            .to_owned();
        pending.abort();
        let _ = pending.await;
        nats.flush().await?;
        let error = nats
            .request(inbox, "0".into())
            .await
            .err()
            .ok_or("the chunk server survived")?;
        assert_eq!(error.kind(), async_nats::client::RequestErrorKind::NoResponders);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn malformed_transfer_descriptors_are_rejected() -> TestResult {
    tokio::time::timeout(DEADLINE, async {
        let (_server, nats) = start_small_payload_server().await?;
        let mut requests = nats.subscribe("test.transfer").await?;
        nats.flush().await?;
        for descriptor in [
            serde_json::json!({"inbox": "_INBOX.test", "length": usize::MAX, "chunk_size": 1024}),
            serde_json::json!({"inbox": "_INBOX.test", "length": 42, "chunk_size": 0}),
            serde_json::json!({"inbox": "sandbox.lease.cancel.secret", "length": 42, "chunk_size": 1024}),
        ] {
            let mut headers = async_nats::HeaderMap::new();
            headers.insert("Sandbox-Transfer", "chunks-v1");
            nats.publish_with_headers("test.transfer", headers, serde_json::to_vec(&descriptor)?.into())
                .await?;
            let message = requests.next().await.ok_or("subscription closed")?;
            assert!(nats_transfer::receive(&nats, &message).await.is_err());
        }
        Ok(())
    })
    .await?
}
