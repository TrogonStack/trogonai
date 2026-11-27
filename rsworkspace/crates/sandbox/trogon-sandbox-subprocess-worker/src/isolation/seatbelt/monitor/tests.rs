use super::*;

const READ_DENIAL: &str = "2026-10-24 01:26:30.661 E  kernel[0:17bbc65b] (Sandbox) Sandbox: cat(74970) deny(1) file-read-data /private/etc/hosts";
const WRITE_DENIAL: &str = "2026-10-24 01:26:30.700 E  kernel[0:17bbc65b] (Sandbox) Sandbox: touch(75001) deny(1) file-write-create /private/tmp/x/.bashrc";
const NETWORK_DENIAL: &str =
    "2026-10-24 01:26:30.689 E  kernel[0:17bbc662] (Sandbox) Sandbox: curl(74972) deny(1) network-outbound remote:*:9";
const IP_NETWORK_DENIAL: &str = "2026-10-24 01:26:30.689 E  kernel[0:17bbc662] (Sandbox) Sandbox: curl(74972) deny(1) network-outbound 93.184.216.34:443";
const DUPLICATES: &str = "2026-10-24 01:26:30.685 E  kernel[0:17bbb9b8] (Sandbox) 4 duplicate reports for Sandbox: curl(74972) deny(1) file-read-data /private/etc/hosts";

#[test]
fn a_file_read_denial_names_the_process_and_path() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_line(READ_DENIAL),
        Some(SeatbeltViolation {
            process: "cat".to_string(),
            pid: 74970,
            operation: "file-read-data".to_string(),
            kind: ViolationKind::FileRead {
                path: "/private/etc/hosts".to_string()
            },
        })
    );
    Ok(())
}

#[test]
fn every_file_write_operation_is_a_write_denial() -> Result<(), Box<dyn std::error::Error>> {
    let violation = parse_line(WRITE_DENIAL).ok_or("a value")?;
    assert_eq!(violation.operation, "file-write-create");
    assert_eq!(
        violation.kind,
        ViolationKind::FileWrite {
            path: "/private/tmp/x/.bashrc".to_string()
        }
    );
    Ok(())
}

#[test]
fn file_denials_become_fs_denials_by_operation() -> Result<(), Box<dyn std::error::Error>> {
    let read = parse_line(READ_DENIAL)
        .ok_or("a value")?
        .to_fs_denial()
        .ok_or("a value")?;
    assert_eq!(read.path.as_str(), "/private/etc/hosts");
    assert_eq!(read.operation, FsOperation::Read);
    let write = parse_line(WRITE_DENIAL)
        .ok_or("a value")?
        .to_fs_denial()
        .ok_or("a value")?;
    assert_eq!(write.path.as_str(), "/private/tmp/x/.bashrc");
    assert_eq!(write.operation, FsOperation::Create);
    let data = parse_line(&WRITE_DENIAL.replace("file-write-create", "file-write-data"))
        .ok_or("a value")?
        .to_fs_denial()
        .ok_or("a value")?;
    assert_eq!(data.operation, FsOperation::Write);
    let unlink = parse_line(&WRITE_DENIAL.replace("file-write-create", "file-write-unlink"))
        .ok_or("a value")?
        .to_fs_denial()
        .ok_or("a value")?;
    assert_eq!(unlink.operation, FsOperation::Delete);
    Ok(())
}

#[test]
fn network_denials_are_not_fs_denials() -> Result<(), Box<dyn std::error::Error>> {
    assert!(parse_line(NETWORK_DENIAL).ok_or("a value")?.to_fs_denial().is_none());
    Ok(())
}

#[test]
fn a_network_denial_with_a_wildcard_host_has_a_port_and_no_address() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_line(NETWORK_DENIAL).ok_or("a value")?.kind,
        ViolationKind::NetworkConnect { ip: None, port: 9 }
    );
    Ok(())
}

#[test]
fn a_network_denial_with_an_address_has_both() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_line(IP_NETWORK_DENIAL).ok_or("a value")?.kind,
        ViolationKind::NetworkConnect {
            ip: Some("93.184.216.34".parse()?),
            port: 443
        }
    );
    Ok(())
}

#[test]
fn repeated_reports_parse_like_the_first() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(parse_line(DUPLICATES).ok_or("a value")?.pid, 74972);
    Ok(())
}

#[test]
fn the_tag_line_and_the_log_header_are_not_denials() -> Result<(), Box<dyn std::error::Error>> {
    assert!(parse_line("TRG_SBX_TESTTAG1").is_none());
    assert!(parse_line("Timestamp               Ty Process[PID:TID]").is_none());
    assert!(parse_line("").is_none());
    Ok(())
}

#[test]
fn known_noise_is_dropped() -> Result<(), Box<dyn std::error::Error>> {
    let line = "2026-10-24 01:26:30.683 E  kernel[0:17bbb9b8] (Sandbox) Sandbox: curl(74972) deny(1) mach-lookup com.apple.diagnosticd";
    assert!(parse_line(line).is_none());
    Ok(())
}

#[test]
fn other_operations_keep_their_detail() -> Result<(), Box<dyn std::error::Error>> {
    let line = "2026-10-24 01:26:30.676 E  kernel[0:17bbc65b] (Sandbox) Sandbox: curl(74972) deny(1) sysctl-read kern.osvariant_status";
    let violation = parse_line(line).ok_or("a value")?;
    assert_eq!(violation.operation, "sysctl-read");
    assert_eq!(
        violation.kind,
        ViolationKind::Other {
            detail: "kern.osvariant_status".to_string()
        }
    );
    Ok(())
}

#[test]
fn a_network_denial_becomes_an_egress_violation() -> Result<(), Box<dyn std::error::Error>> {
    let violation = parse_line(IP_NETWORK_DENIAL).ok_or("a value")?;
    let egress = violation
        .to_egress_violation(LeaseId::new("l"), SystemTime::UNIX_EPOCH)
        .ok_or("an egress violation")?;
    assert_eq!(egress.port, 443);
    assert_eq!(egress.dest_ip, Some("93.184.216.34".parse()?));
    assert_eq!(egress.host, None);
    assert_eq!(egress.reason, EgressViolationReason::PolicyDenied);
    assert!(
        parse_line(READ_DENIAL)
            .ok_or("a value")?
            .to_egress_violation(LeaseId::new("l"), SystemTime::UNIX_EPOCH)
            .is_none()
    );
    Ok(())
}

#[test]
fn only_a_plain_tag_is_put_in_a_predicate() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        predicate(&LogTag::new("TRGSBX_ab12")?).as_deref(),
        Some("eventMessage ENDSWITH \"TRGSBX_ab12\"")
    );
    assert!(predicate(&LogTag::new("a\"b")?).is_none());
    Ok(())
}
