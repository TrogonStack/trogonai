use super::*;

#[tokio::test]
async fn a_native_snapshot_waits_for_the_volume_snapshot_and_names_region_and_image() -> TestResult {
    let env = workspace_env(2000, 1000).await?;
    let lease = lease_id("snapshotted");
    env.provider.lease(&request()?, &lease).await?;
    env.fly.with(|state| {
        state.snapshot_polls_before_ready = 2;
        state.retention_days = Some(7);
        state.snapshot_status = Some("complete".to_owned());
    });
    let outcome = env.provider.native_snapshot(&lease).await.ok_or("not held")?;
    let SnapshotExportOutcome::Native(receipt) = outcome else {
        return Err(format!("unexpected {outcome:?}").into());
    };
    let expected = snapshot_id("iad", IMAGE, "vs_snap1")?;
    assert_eq!(receipt.reference.id.as_str(), expected);
    assert_eq!(receipt.bytes, Some(4096));
    assert!(receipt.reference.expires_at.is_some());
    assert!(env.fly.count("POST", "/exec") >= 2);
    assert!(env.provider.native_snapshot(&lease_id("unknown")).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_snapshot_that_never_completes_fails_as_transient() -> TestResult {
    for case in 0..7 {
        let env = workspace_env(300, 1000).await?;
        let lease = lease_id("slow-snapshot");
        env.provider.lease(&request()?, &lease).await?;
        env.fly.with(|state| match case {
            0 => state.snapshot_never_ready = true,
            1 => state.snapshot_status = Some("failed".to_owned()),
            2 => state.snapshot_status = Some("pending".to_owned()),
            3 => state.exec_exit.push(("/bin/sync".to_owned(), 1)),
            4 => state.snapshot_create_fails = true,
            5 => state.snapshot_without_id = true,
            _ => state.snapshot_omits_status = true,
        });
        let outcome = within(env.provider.native_snapshot(&lease)).await?.ok_or("not held")?;
        assert!(
            matches!(
                outcome,
                SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
            ),
            "case {case}: {outcome:?}"
        );
    }
    Ok(())
}
