use super::*;
use crate::config::{BaseRootfs, FirecrackerBinary};

fn launcher(tmp: &Path) -> Result<JailerLauncher, Box<dyn std::error::Error>> {
    let binary = tmp.join("firecracker");
    std::fs::write(&binary, b"fake")?;
    let worker = FirecrackerConfig::new(
        FirecrackerBinary::new(binary),
        KernelImage::new("/kernel"),
        BaseRootfs::new("/rootfs"),
        StateDir::new(tmp),
    );
    Ok(JailerLauncher::new(
        JailerConfig {
            binary: "/jailer".into(),
            uid_base: NonZeroU32::new(200000).ok_or("zero")?,
            gid_base: NonZeroU32::new(200000).ok_or("zero")?,
            cgroup_parent: "sandbox-fc".into(),
            memory_overhead: MemoryMib::new(128)?,
        },
        &worker,
    )?)
}

#[test]
fn launch_plan_is_confined_and_counts_guest_and_vmm_memory() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let launcher = launcher(tmp.path())?;
    let dir = launcher.vm_dir(&StateDir::new("/s"), &LeaseId::new("lease-1"))?;
    let args: Vec<_> = launcher
        .arguments(&dir, 1, MemoryMib::new(512)?, VcpuCount::new(2)?)?
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    assert!(args.windows(2).any(|pair| pair == ["--uid", "200001"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--parent-cgroup", "sandbox-fc/lease-1"])
    );
    assert!(args.windows(2).any(|pair| pair == ["--cgroup", "memory.max=671088640"]));
    assert!(args.windows(2).any(|pair| pair == ["--cgroup-version", "2"]));
    assert_eq!(&args[args.len() - 3..], ["--", "--api-sock", "/api.sock"]);
    assert!(
        !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--daemonize" | "--new-pid-ns" | "--no-seccomp"))
    );
    assert_eq!(dir.runtime_dir(), Path::new("/s/lease-1/jail/firecracker/vm/root"));
    assert_eq!(
        launcher.kernel_path(&KernelImage::new("/host/kernel")).as_path(),
        Path::new("/vmlinux")
    );
    Ok(())
}

#[tokio::test]
async fn dedicated_identities_are_not_reused_until_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let launcher = launcher(tmp.path())?;
    let a = launcher.vm_dir(&StateDir::new("/s"), &LeaseId::new("a"))?;
    let b = launcher.vm_dir(&StateDir::new("/s"), &LeaseId::new("b"))?;
    assert_eq!(launcher.reserve_identity(&a)?, 0);
    assert_eq!(launcher.reserve_identity(&b)?, 1);
    assert!(launcher.reserve_identity(&a).is_err());
    launcher.cleanup(&a).await?;
    assert_eq!(launcher.reserve_identity(&a)?, 0);
    Ok(())
}

#[test]
fn population_requires_an_explicit_unambiguous_kernel_value() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!cgroup_populated("populated 0\nfrozen 0\n")?);
    assert!(cgroup_populated("frozen 0\npopulated 1\n")?);
    for input in [
        "",
        "frozen 0\n",
        "populated 2\n",
        "populated 0 extra\n",
        "populated 0\npopulated 0\n",
    ] {
        assert!(cgroup_populated(input).is_err(), "{input:?}");
    }
    Ok(())
}

#[tokio::test]
async fn transient_busy_cgroup_is_removed_after_confirmed_emptiness() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let parent = tmp.path().join("lease");
    let leaf = parent.join("vm");
    tokio::fs::create_dir_all(&leaf).await?;
    for path in [&parent, &leaf] {
        tokio::fs::write(path.join("cgroup.events"), "populated 0\n").await?;
    }
    let mut busy = true;
    remove_cgroup_paths_with([leaf.clone(), parent.clone()], Duration::from_secs(2), move |path| {
        let first = std::mem::take(&mut busy);
        async move {
            if first {
                return Err(std::io::Error::from_raw_os_error(libc::EBUSY));
            }
            tokio::fs::remove_file(path.join("cgroup.events")).await?;
            tokio::fs::remove_dir(path).await
        }
    })
    .await?;
    assert!(!leaf.exists());
    assert!(!parent.exists());
    Ok(())
}

#[tokio::test]
async fn cleanup_budget_bounds_a_stalled_removal_without_deleting_the_path() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let parent = tmp.path().join("lease");
    let leaf = parent.join("vm");
    tokio::fs::create_dir_all(&leaf).await?;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        remove_cgroup_paths_with([leaf.clone(), parent.clone()], Duration::from_millis(20), |_| {
            std::future::pending::<std::io::Result<()>>()
        }),
    )
    .await?;
    let error = result.err().ok_or("expected an error")?;
    assert!(format!("{error:?}").contains("exceeded"));
    assert!(leaf.exists());
    assert!(parent.exists());
    Ok(())
}
