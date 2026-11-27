use super::*;

#[test]
fn reserved_cids_are_refused() -> Result<(), ConfigError> {
    for cid in 0..3 {
        assert!(GuestCid::new(cid).is_err(), "cid {cid}");
    }
    assert_eq!(GuestCid::new(3)?, GuestCid::DEFAULT);
    Ok(())
}

#[test]
fn netns_prefixes_are_short_and_shell_safe() -> Result<(), ConfigError> {
    assert_eq!(NetnsPrefix::new("sbxfc-")?.as_str(), "sbxfc-");
    for bad in ["", "a b", "x/y", "a$", "seventeen-chars-x"] {
        assert!(NetnsPrefix::new(bad).is_err(), "{bad:?}");
    }
    Ok(())
}

#[test]
fn zero_values_are_refused() {
    assert!(VsockPort::new(0).is_err());
    assert!(VcpuCount::new(0).is_err());
    assert!(MemoryMib::new(0).is_err());
}

#[test]
fn defaults_follow_the_working_spike() {
    let config = FirecrackerConfig::new(
        FirecrackerBinary::new("/usr/bin/firecracker"),
        KernelImage::new("/k"),
        BaseRootfs::new("/r"),
        StateDir::new("/s"),
    );
    assert_eq!(config.default_vcpus.get(), 2);
    assert_eq!(config.default_memory.get(), 512);
    assert_eq!(config.agent_port, VsockPort::DEFAULT_AGENT);
    assert_eq!(config.boot_args, "console=ttyS0 reboot=k panic=1");
}
