use super::*;

#[test]
fn zero_spec_values_mean_no_cap() -> Result<(), Box<dyn std::error::Error>> {
    let limits = ResourceLimits::from_spec(0, 0);
    assert_eq!(limits.memory, None);
    assert_eq!(limits.cpu_time, None);
    assert_eq!(limits.processes, ProcessLimit::DEFAULT);
    Ok(())
}

#[test]
fn spec_values_convert_to_bytes_and_seconds() -> Result<(), Box<dyn std::error::Error>> {
    let limits = ResourceLimits::from_spec(7, 64);
    assert_eq!(limits.memory.ok_or("memory limit")?.bytes(), 64 * 1024 * 1024);
    assert_eq!(limits.cpu_time.ok_or("cpu limit")?.seconds(), 7);
    Ok(())
}

#[test]
fn vcpu_share_converts_to_a_cpu_max_quota() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(cpu_quota_micros(1000), CPU_PERIOD_MICROS);
    assert_eq!(cpu_quota_micros(500), CPU_PERIOD_MICROS / 2);
    assert_eq!(cpu_quota_micros(4000), CPU_PERIOD_MICROS * 4);
    assert_eq!(cpu_quota_micros(1), 1000);
    Ok(())
}

#[test]
fn rlimit_only_cannot_enforce_a_vcpu_share() -> Result<(), Box<dyn std::error::Error>> {
    let limits = ResourceLimits::from_spec(0, 0).with_vcpu_millis(Some(500));
    let error = ResourceEnforcer::rlimit_only()
        .prepare(&limits)
        .err()
        .ok_or("refused")?;
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(!ResourceEnforcer::rlimit_only().enforces_cpu_quota());
    Ok(())
}

#[test]
fn rlimit_only_reports_rlimit_mechanism() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(ResourceEnforcer::rlimit_only().mechanism(), LimitMechanism::Rlimit);
    assert_eq!(LimitMechanism::CgroupV2.to_string(), "cgroup-v2");
    Ok(())
}

#[test]
fn rlimit_plan_covers_cpu_memory_and_processes() -> Result<(), Box<dyn std::error::Error>> {
    let limits = ResourceLimits::from_spec(3, 16);
    let prepared = ResourceEnforcer::rlimit_only().prepare(&limits)?;
    let resources: Vec<libc::c_int> = prepared.rlimits.iter().map(|s| s.resource).collect();
    assert_eq!(
        resources,
        vec![
            libc::RLIMIT_CPU as libc::c_int,
            libc::RLIMIT_DATA as libc::c_int,
            libc::RLIMIT_NPROC as libc::c_int,
        ]
    );
    Ok(())
}

fn delegation_in(dir: &Path) -> ResourceEnforcer {
    ResourceEnforcer {
        cgroup: Some(CgroupDelegation {
            leases: dir.to_path_buf(),
            cpu: false,
        }),
    }
}

#[test]
fn a_launch_sharing_a_cgroup_joins_it_instead_of_making_one() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let shared = dir.path().join("lease");
    fs::create_dir(&shared)?;
    fs::write(shared.join("cgroup.procs"), "")?;

    let limits = ResourceLimits::from_spec(0, 0).sharing_cgroup(Some(shared.clone()));
    let guard = delegation_in(dir.path()).prepare(&limits)?.into_guard();

    assert_eq!(guard.cgroup_path(), Some(shared.clone()));
    let made = fs::read_dir(dir.path())?.count();
    assert_eq!(made, 1, "no cgroup of its own was created");
    assert!(!shared.join("pids.max").exists(), "limits were not rewritten");
    Ok(())
}

#[tokio::test]
async fn releasing_a_joined_cgroup_leaves_it_to_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let shared = dir.path().join("lease");
    fs::create_dir(&shared)?;
    fs::write(shared.join("cgroup.procs"), "")?;

    let limits = ResourceLimits::from_spec(0, 0).sharing_cgroup(Some(shared.clone()));
    let guard = delegation_in(dir.path()).prepare(&limits)?.into_guard();
    guard.release().await;

    assert!(shared.is_dir());
    assert!(!shared.join("cgroup.kill").exists());
    Ok(())
}

#[test]
fn sharing_a_cgroup_that_is_gone_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let limits = ResourceLimits::from_spec(0, 0).sharing_cgroup(Some(dir.path().join("gone")));
    let error = delegation_in(dir.path()).prepare(&limits).err().ok_or("refused")?;
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    Ok(())
}
