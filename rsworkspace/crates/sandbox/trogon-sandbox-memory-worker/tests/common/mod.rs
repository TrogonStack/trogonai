//! A memory worker on a throwaway NATS server, driven over its provider subjects.

#![allow(dead_code)]

use std::time::Duration;

use async_nats::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trogon_sandbox_memory_worker::{CommandTable, MemoryWorker, MemoryWorkerConfig};

use crate::nats_common::Server;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub struct Harness {
    pub server: Server,
    pub client: Client,
    pub worker: MemoryWorker,
}

pub async fn start() -> Result<Harness, Box<dyn std::error::Error>> {
    start_with(CommandTable::default()).await
}

pub async fn start_with(commands: CommandTable) -> Result<Harness, Box<dyn std::error::Error>> {
    let server = Server::start().await;
    let client = server.client().await?;
    let worker = MemoryWorker::start(
        client.clone(),
        MemoryWorkerConfig {
            commands,
            ..Default::default()
        },
    )
    .await?;
    Ok(Harness { server, client, worker })
}

pub fn lease_request() -> Value {
    json!({
        "tenant": "acme",
        "provider_pref": ["mock"],
        "spec": {
            "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
            "workspace": { "empty": {} },
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
            "privileged": false,
            "boot_mode": "entrypoint_default",
        },
        "ttl_seconds": 60,
        "idempotency_key": "key-1",
    })
}

impl Harness {
    /// Sends the dispatch for `lease` and returns the raw reply.
    pub async fn dispatch_with(&self, lease: &str, request: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let payload = json!({ "lease_id": lease, "request": request });
        let reply = self
            .client
            .request("sandbox.provider.mock.lease", serde_json::to_vec(&payload)?.into())
            .await?;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    /// Grants `lease` and checks the grant names this worker.
    pub async fn grant(&self, lease: &str) -> TestResult {
        let reply = self.dispatch_with(lease, lease_request()).await?;
        assert_eq!(reply["worker_id"], self.worker.worker_id().as_str());
        Ok(())
    }

    /// Sends `body` on the `verb` subject of `lease`.
    pub async fn call_raw(&self, verb: &str, lease: &str, body: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let subject = format!("sandbox.provider.mock.{verb}.{lease}");
        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            self.client.request(subject, serde_json::to_vec(&body)?.into()),
        )
        .await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    /// Like [`Harness::call_raw`], read as the verb's typed response.
    pub async fn call<T: DeserializeOwned>(
        &self,
        verb: &str,
        lease: &str,
        body: Value,
    ) -> Result<T, Box<dyn std::error::Error>> {
        Ok(serde_json::from_value(self.call_raw(verb, lease, body).await?)?)
    }
}
