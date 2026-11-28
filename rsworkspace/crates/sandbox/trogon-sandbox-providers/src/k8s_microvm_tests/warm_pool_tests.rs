use super::*;

/// The race the fence exists for: two workers list the same idle pod, both
/// decide to claim it, and both write. Without a `resourceVersion` in the
/// patch both writes are accepted and both workers believe they own the
/// pod -- one lease silently running inside another tenant's sandbox.
#[tokio::test]
async fn a_second_claim_at_the_same_resource_version_is_refused() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    mock.create_pod(
        "default",
        Pod {
            metadata: ObjectMeta {
                name: Some("warm-1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await?;

    // Both workers observed the pod in the same state.
    let observed = mock.list_pods("default", "").await?;
    let resource_version = observed.pods[0]
        .metadata
        .resource_version
        .clone()
        .ok_or("missing value")?;

    let first = mock
        .patch_pod_labels(
            "default",
            "warm-1",
            BTreeMap::from([(LABEL_LEASE_ID.to_string(), "lease-a".to_string())]),
            &resource_version,
        )
        .await?;
    let second = mock
        .patch_pod_labels(
            "default",
            "warm-1",
            BTreeMap::from([(LABEL_LEASE_ID.to_string(), "lease-b".to_string())]),
            &resource_version,
        )
        .await?;

    assert_eq!(first, FencedPatchOutcome::Applied);
    assert_eq!(second, FencedPatchOutcome::PreconditionFailed);
    assert_eq!(
        mock.pod_named("warm-1")?.metadata.labels.ok_or("missing value")?[LABEL_LEASE_ID],
        "lease-a",
        "the loser must not overwrite the winner's lease"
    );
    Ok(())
}

#[tokio::test]
async fn a_warm_pod_labels_a_valid_digest_and_annotates_the_full_one() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let digest = format!("sha256:{}", "cd".repeat(32));
    let pod_name = provider(mock.clone())
        .create_warm_pod(&digest, "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;

    let pod = mock.pod_named(&pod_name)?;
    let labels = pod.metadata.labels.ok_or("missing value")?;
    assert!(is_valid_label_value(&labels[LABEL_DIGEST]));
    assert_eq!(
        pod.metadata.annotations.ok_or("missing value")?[ANNOTATION_DIGEST],
        digest
    );
    Ok(())
}

#[tokio::test]
async fn a_warm_pod_records_where_its_snapshot_came_from() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let pod_name = provider(mock.clone())
        .create_warm_pod_from(
            "sha256:abc",
            Some(("registry.test", "snapshots/tenant-a")),
            "img:1",
            "restore-1",
            "kata-fc",
            0.25,
            256,
        )
        .await?;

    let annotations = mock.pod_named(&pod_name)?.metadata.annotations.ok_or("missing value")?;
    assert_eq!(annotations[ANNOTATION_SOURCE_REGISTRY], "registry.test");
    assert_eq!(annotations[ANNOTATION_SOURCE_REPOSITORY], "snapshots/tenant-a");
    Ok(())
}

#[tokio::test]
async fn claim_warm_pod_fences_its_write_and_labels_the_lease() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let p = provider(mock.clone());
    let pod_name = p
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();
    let before = mock.pod_named(&pod_name)?.metadata.resource_version;

    let claimed = p
        .claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::ZERO)
        .await?;

    assert_eq!(claimed.map(|c| c.name).as_deref(), Some(pod_name.as_str()));
    let after = mock.pod_named(&pod_name)?;
    let labels = after.metadata.labels.ok_or("missing value")?;
    assert_eq!(labels[LABEL_WARM_POOL], WARM_POOL_CLAIMED);
    assert_eq!(labels[LABEL_LEASE_ID], "lease-a");
    assert_eq!(labels[LABEL_TENANT_ID], "tenant-a");
    assert_ne!(
        after.metadata.resource_version, before,
        "an accepted fenced write advances the resourceVersion"
    );
    Ok(())
}

/// A warm pod's restore Secret has a generated name, so a worker that
/// restarted between creating the pod and claiming it must learn the name
/// from the pod rather than guess it.
#[tokio::test]
async fn a_claim_reports_the_restore_secret_the_pod_mounts() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let p = provider(mock.clone());
    p.create_warm_pod("sha256:abc", "img:1", "warm-restore-01abc", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();

    let claimed = p
        .claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::ZERO)
        .await?
        .ok_or("missing value")?;

    assert_eq!(claimed.restore_secret_name.as_deref(), Some("warm-restore-01abc"));
    Ok(())
}

/// Deleting a warm pod by any path, including one that never learned the
/// Secret's name, must take its restore Secret with it.
#[tokio::test]
async fn a_warm_pod_owns_its_restore_secret() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let pod_name = provider(mock.clone())
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;

    let owners = mock.secret_owners();
    assert_eq!(owners.len(), 1);
    let (secret, owner) = &owners[0];
    assert_eq!(secret, "restore-1");
    assert_eq!(owner.kind, "Pod");
    assert_eq!(owner.api_version, "v1");
    assert_eq!(owner.name, pod_name);
    assert_eq!(Some(owner.uid.clone()), mock.pod_named(&pod_name)?.metadata.uid);
    Ok(())
}

/// A warm pod is created with no `NetworkPolicy`, so a claim that does not
/// apply the lease's policy hands out a pod with open egress.
#[tokio::test]
async fn a_claimed_warm_pod_gets_the_deny_all_policy_selecting_only_its_lease() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let p = provider(mock.clone());
    let pod_name = claimed_warm_pod(&mock, &p).await?;

    let name = p
        .apply_network_policy_to_claimed_warm_pod(&pod_name, "lease-a", &NetworkPolicy::DenyAll)
        .await?;

    let created = mock.created_network_policies();
    assert_eq!(created.len(), 1);
    assert_eq!(name, created[0].metadata.name);
    let spec = created[0].spec.as_ref().ok_or("missing value")?;
    assert_eq!(spec.egress, Some(vec![]), "an empty rule list denies all egress");
    assert_eq!(
        spec.pod_selector.as_ref().ok_or("missing value")?.match_labels,
        Some(BTreeMap::from([(LABEL_LEASE_ID.to_string(), "lease-a".to_string())]))
    );
    Ok(())
}

#[tokio::test]
async fn a_claimed_warm_pod_with_an_open_policy_gets_no_network_policy() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let p = provider(mock.clone());
    let pod_name = claimed_warm_pod(&mock, &p).await?;

    let name = p
        .apply_network_policy_to_claimed_warm_pod(&pod_name, "lease-a", &NetworkPolicy::AllowAll)
        .await?;

    assert_eq!(name, None);
    assert!(mock.created_network_policies().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_claimed_warm_pod_refuses_a_policy_only_the_egress_proxy_can_enforce() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    let p = provider(mock.clone());
    let pod_name = claimed_warm_pod(&mock, &p).await?;
    let policy = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };

    let result = p
        .apply_network_policy_to_claimed_warm_pod(&pod_name, "lease-a", &policy)
        .await;

    assert!(result.is_err());
    assert!(mock.created_network_policies().is_empty());
    Ok(())
}

/// A pod with no `resourceVersion` cannot be claimed exclusively, so it is
/// left alone rather than claimed unfenced.
#[tokio::test]
async fn claim_warm_pod_skips_a_pod_it_cannot_fence() -> TestResult {
    let mock = MockK8sOps::new(vec![MockResponse::Ok]);
    mock.seed_pod(Pod {
        metadata: ObjectMeta {
            name: Some("unfenceable".to_string()),
            labels: Some(BTreeMap::from([(
                LABEL_WARM_POOL.to_string(),
                WARM_POOL_IDLE.to_string(),
            )])),
            ..Default::default()
        },
        status: Some(k8s_openapi::api::core::v1::PodStatus {
            phase: Some("Running".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    });

    let claimed = provider(mock.clone())
        .claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::ZERO)
        .await?;

    assert_eq!(claimed, None);
    assert_eq!(
        mock.pod_named("unfenceable")?.metadata.labels.ok_or("missing value")?[LABEL_WARM_POOL],
        WARM_POOL_IDLE
    );
    Ok(())
}
