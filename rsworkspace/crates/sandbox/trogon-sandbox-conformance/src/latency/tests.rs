use super::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn percentile_is_nearest_rank() {
    let samples: Vec<_> = (1..=20).map(ms).collect();
    assert_eq!(percentile(&samples, 50.0), Some(ms(10)));
    assert_eq!(percentile(&samples, 95.0), Some(ms(19)));
    assert_eq!(percentile(&samples, 100.0), Some(ms(20)));
    assert_eq!(percentile(&samples, 0.0), Some(ms(1)));
}

#[test]
fn percentile_of_one_sample_is_that_sample() {
    assert_eq!(percentile(&[ms(7)], 95.0), Some(ms(7)));
    assert_eq!(percentile(&[], 50.0), None);
}

#[test]
fn percentile_clamps_ranks_outside_the_samples() {
    let samples = [ms(1), ms(2)];
    assert_eq!(percentile(&samples, 150.0), Some(ms(2)));
    assert_eq!(percentile(&samples, -10.0), Some(ms(1)));
    assert_eq!(percentile(&samples, f64::NAN), Some(ms(1)));
}

#[test]
fn table_shows_percentiles_skips_and_failures() {
    let mut ran = LatencyStep::new("create");
    ran.samples = vec![ms(30), ms(10), ms(20)];
    ran.failures = 1;
    let mut skipped = LatencyStep::new("snapshot");
    skipped.skip("no snapshot registry given");
    skipped.skip("a later reason is not kept");
    let mut both = LatencyStep::new("delete");
    both.skip("unsupported: no");
    both.failures = 2;
    let table = render_latency(&[LatencyReport {
        provider: "mock".into(),
        steps: vec![ran, skipped, both],
    }]);
    let lines: Vec<_> = table.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].starts_with("provider  step"));
    assert!(lines[0].ends_with("p95 ms  note"));
    assert!(lines[1].contains("20.0") && lines[1].contains("30.0"));
    assert!(lines[1].ends_with("1 failed"));
    assert!(lines[2].contains(" - ") && lines[2].ends_with("no snapshot registry given"));
    assert!(lines[3].ends_with("unsupported: no; 2 failed"));
}

#[test]
fn a_push_target_carries_a_rootfs_artifact() {
    let target = snapshot_push_target("registry.test", "latency/snapshots");
    assert_eq!(target.registry, "registry.test");
    assert_eq!(target.repository, "latency/snapshots");
    assert_eq!(target.artifact_type, ArtifactType::SandboxRootfsV1);
}
