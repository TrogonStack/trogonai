use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_providers::{BollardEngine, LocalDockerProvider, SandboxProvider};

use super::support::fake_docker::FakeDocker;
use super::support::sample;

type TestResult = Result<(), Box<dyn Error>>;

const TIMEOUT: Duration = Duration::from_secs(30);
const SIDECAR_IMAGE: &str = "proxy:1";

async fn provider() -> Result<(FakeDocker, LocalDockerProvider), Box<dyn Error>> {
    let daemon = FakeDocker::start().await?;
    let engine = BollardEngine::connect_unix(daemon.socket_path())?;
    let provider = LocalDockerProvider::new(Arc::new(engine), SIDECAR_IMAGE.to_string());
    Ok((daemon, provider))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_lease_user_wins_over_the_provider_default_user() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let provider = provider.with_default_user(Some("1000:1000".to_string()));
        let mut spec = sample::spec();
        spec.user = Some("2000".to_string());

        provider.launch(spec).await?;

        let creates = daemon.create_requests();
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].1["User"], "2000");
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn the_provider_default_user_applies_when_the_lease_names_none() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let provider = provider.with_default_user(Some("1000:1000".to_string()));

        provider.launch(sample::spec()).await?;

        let creates = daemon.create_requests();
        assert_eq!(creates[0].1["User"], "1000:1000");
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn the_egress_sidecar_is_created_with_a_minimal_capability_and_resource_spec() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::intercepting_policy()?);
        spec.lease_id = Some(LeaseId::new("lease-sidecar"));

        let handle = provider.launch(spec).await?;

        let creates = daemon.create_requests();
        assert_eq!(creates.len(), 2);
        let host = &creates[1].1["HostConfig"];
        assert_eq!(strings(&host["CapDrop"]), vec!["ALL"]);
        let mut added = strings(&host["CapAdd"]);
        added.sort();
        assert_eq!(added, vec!["NET_ADMIN", "SETGID", "SETPCAP", "SETUID"]);
        assert_eq!(host["PidsLimit"], 256);
        let memory = 128_i64 * 1024 * 1024;
        assert_eq!(host["Memory"], memory);
        assert_eq!(host["MemorySwap"], memory);
        assert_eq!(host["ReadonlyRootfs"], true);
        assert_eq!(strings(&host["SecurityOpt"]), vec!["no-new-privileges"]);

        if let Some(path) = handle.policy_file_path() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    })
    .await?
}
