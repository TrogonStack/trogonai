use super::*;

fn tree(path: &str) -> Result<WriteGrant, Box<dyn std::error::Error>> {
    Ok(WriteGrant::tree(LiteralPath::new(path)?))
}

fn existing(path: &str) -> Result<WriteGrant, Box<dyn std::error::Error>> {
    Ok(WriteGrant::existing_files(LiteralPath::new(path)?))
}

fn roots(plan: &WritePlan) -> Vec<&str> {
    plan.grants().iter().map(|g| g.root.as_str()).collect()
}

#[test]
fn kernel_versions_map_to_support_levels() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(LandlockSupport::from_kernel_version(-1), LandlockSupport::Unavailable);
    assert_eq!(LandlockSupport::from_kernel_version(0), LandlockSupport::Unavailable);
    assert_eq!(
        LandlockSupport::from_kernel_version(1),
        LandlockSupport::Partial(LandlockAbi(1))
    );
    assert_eq!(
        LandlockSupport::from_kernel_version(3),
        LandlockSupport::Full(LandlockAbi(3))
    );
    assert_eq!(
        LandlockSupport::from_kernel_version(7),
        LandlockSupport::Full(LandlockAbi(7))
    );
    Ok(())
}

#[test]
fn support_names_the_level_that_applies() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(LandlockSupport::Unavailable.to_string(), "none");
    assert_eq!(
        LandlockSupport::Partial(LandlockAbi(2)).to_string(),
        "landlock-v2-partial"
    );
    assert_eq!(LandlockSupport::Full(LandlockAbi(5)).to_string(), "landlock-v5");
    assert!(!LandlockSupport::Unavailable.is_enforced());
    assert!(LandlockSupport::Partial(LandlockAbi(1)).is_enforced());
    Ok(())
}

#[test]
fn older_abis_handle_fewer_rights() -> Result<(), Box<dyn std::error::Error>> {
    let v1 = LandlockAbi(1).handled_access();
    let v3 = LandlockAbi(3).handled_access();
    assert!(!v1.contains(AccessFs::Refer));
    assert!(!v1.contains(AccessFs::Truncate));
    assert!(v3.contains(AccessFs::Refer | AccessFs::Truncate));
    Ok(())
}

#[test]
fn rights_outside_the_mounts_are_never_handled() -> Result<(), Box<dyn std::error::Error>> {
    for version in 1..=9 {
        let handled = LandlockAbi(version).handled_access();
        assert!(!handled.intersects(unhandled()));
        assert!(handled.contains(AccessFs::WriteFile | AccessFs::MakeReg));
    }
    Ok(())
}

#[test]
fn nested_trees_collapse_into_the_outer_root() -> Result<(), Box<dyn std::error::Error>> {
    let plan = WritePlan::new([tree("/workspace/out")?, tree("/workspace")?, tree("/workspace")?]);
    assert_eq!(roots(&plan), ["/workspace"]);
    Ok(())
}

#[test]
fn existing_file_grants_inside_a_tree_are_redundant() -> Result<(), Box<dyn std::error::Error>> {
    let plan = WritePlan::new([existing("/dev/shm")?, tree("/dev")?]);
    assert_eq!(roots(&plan), ["/dev"]);
    assert_eq!(plan.grants()[0].scope, WriteScope::Tree);
    Ok(())
}

#[test]
fn a_tree_inside_an_existing_file_grant_is_kept() -> Result<(), Box<dyn std::error::Error>> {
    let plan = WritePlan::new([existing("/dev")?, tree("/dev/shm")?]);
    assert_eq!(roots(&plan), ["/dev", "/dev/shm"]);
    Ok(())
}

#[test]
fn siblings_sharing_a_prefix_stay_separate() -> Result<(), Box<dyn std::error::Error>> {
    let plan = WritePlan::new([tree("/work")?, tree("/workspace")?]);
    assert_eq!(roots(&plan), ["/work", "/workspace"]);
    Ok(())
}

#[test]
fn existing_file_scope_excludes_creation_and_removal() -> Result<(), Box<dyn std::error::Error>> {
    let handled = LandlockAbi(3).handled_access();
    let access = WriteScope::ExistingFiles.access(handled);
    assert_eq!(access, AccessFs::WriteFile | AccessFs::Truncate);
    assert_eq!(WriteScope::Tree.access(handled), handled);
    Ok(())
}

#[test]
fn wrapped_command_carries_the_abi_and_every_root() -> Result<(), Box<dyn std::error::Error>> {
    let confinement = Confinement::new(LandlockAbi(4), WritePlan::new([tree("/workspace")?, existing("/dev")?]));
    let (launcher, args) = confinement.wrap_command(
        GUEST_LAUNCHER_BINARY,
        Path::new("/bin/sh"),
        &["-c".to_string(), "true".to_string()],
    );
    assert_eq!(launcher, Path::new(GUEST_LAUNCHER_BINARY));
    assert_eq!(
        args,
        [
            "--confine",
            "--abi",
            "4",
            "--write-existing",
            "/dev",
            "--write",
            "/workspace",
            "--",
            "/bin/sh",
            "-c",
            "true"
        ]
    );
    Ok(())
}

#[test]
fn the_launcher_flag_is_recognised_in_position_one_only() -> Result<(), Box<dyn std::error::Error>> {
    let argv = |parts: &[&str]| parts.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    assert!(is_confine_invocation(&argv(&["worker", "--confine"])));
    assert!(!is_confine_invocation(&argv(&["worker", "--relay"])));
    assert!(!is_confine_invocation(&argv(&["worker"])));
    Ok(())
}
