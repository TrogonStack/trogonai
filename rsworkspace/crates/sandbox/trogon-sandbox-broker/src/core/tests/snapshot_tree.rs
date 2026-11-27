use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn snapshot_tree_walks_parents_with_their_recorded_times() -> TestResult {
    let nodes = tree_of(
        vec![
            tree_record("t", 3, Some(2))?,
            tree_record("t", 2, Some(1))?,
            tree_record("t", 1, None)?,
        ],
        "t",
        3,
    )
    .await?;
    let digests: Vec<_> = nodes.iter().map(|n| n.digest.clone()).collect();
    assert_eq!(digests, vec![tree_digest(3)?, tree_digest(2)?, tree_digest(1)?]);
    assert_eq!(nodes[0].parent, Some(tree_digest(2)?));
    assert_eq!(nodes[2].parent, None);
    assert_eq!(nodes[2].created_at, SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    Ok(())
}

#[tokio::test]
async fn snapshot_tree_stops_at_a_foreign_tenant_parent() -> TestResult {
    let nodes = tree_of(
        vec![tree_record("t", 2, Some(1))?, tree_record("other", 1, None)?],
        "t",
        2,
    )
    .await?;
    assert_eq!(nodes.len(), 1);
    Ok(())
}

#[tokio::test]
async fn snapshot_tree_stops_on_a_cycle() -> TestResult {
    let nodes = tree_of(
        vec![tree_record("t", 1, Some(2))?, tree_record("t", 2, Some(1))?],
        "t",
        1,
    )
    .await?;
    assert_eq!(nodes.len(), 2);
    Ok(())
}

#[tokio::test]
async fn snapshot_tree_bounds_the_parent_walk() -> TestResult {
    let total = MAX_SNAPSHOT_TREE_DEPTH + 10;
    let records = (1..=total)
        .map(|n| tree_record("t", n, (n > 1).then(|| n - 1)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let nodes = tree_of(records, "t", total).await?;
    assert_eq!(nodes.len(), MAX_SNAPSHOT_TREE_DEPTH);
    Ok(())
}

#[test]
fn snapshot_kv_records_without_lineage_still_load() -> TestResult {
    let json = serde_json::json!({
        "lease_id": "l",
        "tenant": "t",
        "digest": tree_digest(1)?,
        "artifact_type": "sandbox_rootfs_v1",
        "registry": "r",
        "repository": "p",
    });
    let record: SnapshotKvRecord = serde_json::from_value(json)?;
    assert!(record.parent.is_none() && record.created_at.is_none());
    Ok(())
}

#[tokio::test]
async fn snapshot_get_returns_not_found_for_unknown_digest() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let digest = Digest::from_str(&format!("sha256:{}", "d".repeat(64)))?;

    let response = core
        .handle_snapshot_get(SnapshotGetRequest {
            tenant: TenantId::new("test-tenant")?,
            digest,
        })
        .await;

    assert_eq!(response, SnapshotGetResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn snapshot_get_returns_found_for_known_digest_in_matching_tenant() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let digest = Digest::from_str(&format!("sha256:{}", "e".repeat(64)))?;
    let snapshot_kv = Arc::new(InMemorySnapshotKvReader::new());
    snapshot_kv.put(SnapshotKvRecord {
        lease_id: LeaseId::new("source-lease"),
        tenant: tenant.clone(),
        digest: digest.clone(),
        artifact_type: ArtifactType::SandboxRootfsV1,
        registry: "registry.example.com".to_string(),
        repository: "snapshots/test-tenant".to_string(),
        parent: None,
        created_at: None,
        env_digest: None,
    });
    let core = make_core(mock_provider_response()?)?.with_snapshot_kv_reader(snapshot_kv);

    let response = core
        .handle_snapshot_get(SnapshotGetRequest {
            tenant: tenant.clone(),
            digest: digest.clone(),
        })
        .await;

    match response {
        SnapshotGetResponse::Found { row } => {
            assert_eq!(row.digest, digest);
            assert_eq!(row.tenant, tenant);
            assert_eq!(row.artifact_type, ArtifactType::SandboxRootfsV1);
            assert_eq!(row.parent, None);
        }
        other => return Err(format!("expected Found, got {other:?}").into()),
    }
    Ok(())
}
