use trogon_sandbox_contract::{ExtraCapability, PidsLimit, RootfsOverride, ShmSizeMib};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn base() -> ContainerHardening {
    let mut hardening = ContainerHardening::secure();
    hardening.readonly_rootfs = true;
    hardening
}

#[test]
fn no_overrides_leave_the_hardening_untouched() {
    assert_eq!(
        hardening_with_overrides(&base(), &ContainerOverrides::default()),
        base()
    );
}

#[test]
fn overrides_replace_only_what_they_name() -> TestResult {
    let overrides = ContainerOverrides {
        pids_limit: Some(PidsLimit::new(10_000)?),
        writable_rootfs: RootfsOverride::Writable,
        shm_size_mib: Some(ShmSizeMib::new(2048)?),
        extra_capabilities: vec![ExtraCapability::new("NET_RAW")?],
    };
    let applied = hardening_with_overrides(&base(), &overrides);
    assert_eq!(applied.pids_limit, Some(10_000));
    assert!(!applied.readonly_rootfs);
    assert_eq!(applied.shm_size_mib, Some(2048));
    assert!(applied.retained_capabilities.contains(&"NET_RAW".to_string()));
    assert!(applied.no_new_privileges);
    assert!(applied.drop_all_capabilities);
    assert!(applied.disable_swap);
    let (add, drop) = applied.capability_sets(&[]);
    assert!(add.contains(&"NET_RAW".to_string()));
    assert_eq!(drop, vec!["ALL"]);
    Ok(())
}

#[test]
fn a_forbidden_capability_cannot_be_parsed_into_an_override() {
    assert!(ExtraCapability::new("SYS_ADMIN").is_err());
    assert!(ExtraCapability::new("ALL").is_err());
}

#[test]
fn a_capability_already_retained_is_not_added_twice() -> TestResult {
    let overrides = ContainerOverrides {
        extra_capabilities: vec![ExtraCapability::new("CHOWN")?],
        ..Default::default()
    };
    assert_eq!(hardening_with_overrides(&base(), &overrides), base());
    Ok(())
}
