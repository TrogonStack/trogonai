use super::*;

fn probe(platform: &str, architecture: &str, major: u32, minor: u32) -> String {
    format!(r#"{{"platform":"{platform}","architecture":"{architecture}","node_major":{major},"node_minor":{minor}}}"#)
}

#[test]
fn linux_with_a_new_enough_node_is_accepted() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        GuestPlatform::parse(&probe("linux", "arm64", 22, 7))?.architecture,
        GuestArchitecture::Arm64
    );
    assert_eq!(
        GuestPlatform::parse(&probe("linux", "x64", 24, 0))?.architecture,
        GuestArchitecture::X64
    );
    Ok(())
}

#[test]
fn an_old_node_or_another_platform_is_refused() {
    for output in [
        probe("linux", "x64", 22, 6),
        probe("linux", "x64", 20, 11),
        probe("darwin", "x64", 24, 0),
    ] {
        assert!(matches!(
            GuestPlatform::parse(&output),
            Err(OrdinaryCommandRuntimeError::GuestUnsupported)
        ));
    }
}

#[test]
fn malformed_or_padded_probe_output_is_refused() {
    for output in ["", "not json", r#"{"platform":"linux"}"#] {
        assert!(matches!(
            GuestPlatform::parse(output),
            Err(OrdinaryCommandRuntimeError::GuestIdentityMalformed)
        ));
    }
    let padded = probe("linux", "x64", 24, 0).replace('}', r#","extra":1}"#);
    assert!(GuestPlatform::parse(&padded).is_err());
}

#[test]
fn an_unknown_architecture_is_refused() {
    assert!(matches!(
        GuestPlatform::parse(&probe("linux", "riscv64", 24, 0)),
        Err(OrdinaryCommandRuntimeError::GuestArchitectureUnsupported)
    ));
}
