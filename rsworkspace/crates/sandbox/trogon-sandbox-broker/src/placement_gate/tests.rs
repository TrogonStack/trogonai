use std::error::Error;
use std::str::FromStr;

use trogon_sandbox_contract::{ArtifactType, LeaseId, SnapshotKvRecord, TenantId};

use super::*;
use crate::core::InMemorySnapshotKvReader;

type TestResult = Result<(), Box<dyn Error>>;

fn digest(c: char) -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::from_str(&format!("sha256:{}", c.to_string().repeat(64)))?)
}

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("t")?)
}

fn record(snapshot: char, env: Option<char>) -> Result<SnapshotKvRecord, Box<dyn Error>> {
    Ok(SnapshotKvRecord {
        lease_id: LeaseId::new("l"),
        tenant: tenant()?,
        digest: digest(snapshot)?,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
        registry: "r".into(),
        repository: "snap".into(),
        parent: None,
        created_at: None,
        env_digest: env
            .map(|c| EnvDigest::new(format!("sha256:{}", c.to_string().repeat(64))))
            .transpose()?,
    })
}

#[tokio::test]
async fn the_digest_comes_from_the_catalog_record() -> TestResult {
    let catalog = InMemorySnapshotKvReader::new();
    catalog.put(record('1', Some('a'))?);
    catalog.put(record('2', None)?);
    let t = tenant()?;
    let derived = derive_env_digest(&catalog, &t, Some(&digest('1')?))
        .await
        .ok_or("env digest not derived")?;
    assert_eq!(derived.as_str(), format!("sha256:{}", "a".repeat(64)));
    assert!(derive_env_digest(&catalog, &t, Some(&digest('2')?)).await.is_none());
    assert!(derive_env_digest(&catalog, &t, Some(&digest('3')?)).await.is_none());
    assert!(derive_env_digest(&catalog, &t, None).await.is_none());
    Ok(())
}
