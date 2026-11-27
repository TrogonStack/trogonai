use super::*;

#[test]
fn the_sysctl_allowlist_is_the_expected_one() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(SYSCTL_NAMES.len(), 49);
    assert_eq!(SYSCTL_PREFIXES.len(), 9);
    for name in [
        "kern.iossupportversion",
        "hw.ephemeral_storage",
        "kern.hv_support",
        "kern.monotonicclock_usecs",
    ] {
        assert!(!SYSCTL_NAMES.contains(&name), "{name}");
    }
    assert!(!SYSCTL_NAMES.contains(&"kern.proc.all"));
    assert!(SYSCTL_PREFIXES.contains(&"kern.proc.all"));
    Ok(())
}

#[test]
fn the_baseline_builds() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!baseline_allows()?.is_empty());
    Ok(())
}

#[test]
fn the_baseline_grants_no_file_read_or_network_access() -> Result<(), Box<dyn std::error::Error>> {
    let text = format!("{:?}", baseline_allows()?);
    for operation in ["FileReadStar", "NetworkStar", "NetworkOutbound"] {
        assert!(!text.contains(operation), "{operation}");
    }
    Ok(())
}

#[test]
fn the_baseline_writes_only_the_standard_stream_and_terminal_devices() -> Result<(), Box<dyn std::error::Error>> {
    let writes: Vec<String> = baseline_allows()?
        .iter()
        .map(|rule| format!("{rule:?}"))
        .filter(|rule| rule.contains("FileWriteStar"))
        .collect();
    assert_eq!(writes.len(), 1);
    for device in DEFAULT_WRITE_DEVICES {
        assert!(writes[0].contains(device), "{device}");
    }
    assert!(!writes[0].contains("Regex") && !writes[0].contains("Subpath"));
    Ok(())
}
