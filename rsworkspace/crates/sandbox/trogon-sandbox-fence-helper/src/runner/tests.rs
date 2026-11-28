use super::*;

#[test]
fn cmd_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let s = RunnerCmd::Exec(RunnerSpec {
        argv: vec!["cmd.exe".into(), "/c".into(), "echo hi".into()],
        env_overlay: vec![("PATH".into(), r"C:\a;C:\b".into())],
        write_restricted_token: true,
        capability_sids: vec!["S-1-0-1953719907-1-2-3-4".into()],
    });
    let bytes = encode_cmd(&s)?;
    assert_eq!(u32::from_le_bytes(bytes[..4].try_into()?) as usize, bytes.len() - 4);
    let back: RunnerCmd = serde_json::from_slice(&bytes[4..])?;
    match back {
        RunnerCmd::Exec(r) => {
            assert_eq!(r.argv, ["cmd.exe", "/c", "echo hi"]);
            assert_eq!(r.env_overlay.len(), 1);
            assert!(r.write_restricted_token);
            assert_eq!(r.capability_sids, ["S-1-0-1953719907-1-2-3-4"]);
        }
        _ => return Err("wrong variant".into()),
    }
    // A spec that leaves the token mode out is refused, not run with the
    // ordinary token.
    let no_mode = br#"{"Exec":{"argv":["cmd.exe"],"env_overlay":[]}}"#;
    assert!(serde_json::from_slice::<RunnerCmd>(no_mode).is_err());
    // So is a misspelled field, which would otherwise be dropped.
    let typo =
        br#"{"Exec":{"argv":["cmd.exe"],"env_overlay":[],"write_restricted_token":true,"capabilty_sids":["x"]}}"#;
    assert!(serde_json::from_slice::<RunnerCmd>(typo).is_err());
    let explicit = br#"{"Exec":{"argv":["cmd.exe"],"env_overlay":[],"write_restricted_token":false}}"#;
    match serde_json::from_slice::<RunnerCmd>(explicit)? {
        RunnerCmd::Exec(r) => {
            assert!(!r.write_restricted_token);
            assert!(r.capability_sids.is_empty());
        }
        _ => return Err("wrong variant".into()),
    }
    let ca = RunnerCmd::InstallCa {
        der: crate::cert_store::CertDer::raw(vec![0x30, 0x82]),
    };
    let bytes = encode_cmd(&ca)?;
    let back: RunnerCmd = serde_json::from_slice(&bytes[4..])?;
    assert!(matches!(
        back, RunnerCmd::InstallCa { der } if der.as_bytes() == [0x30, 0x82]
    ));
    let probe = RunnerCmd::ProbeEgress {
        target: "127.0.0.1:49999".into(),
        anonymous_target: Some("192.0.2.7:49999".into()),
        anonymous_egress: true,
        allow_loopback: true,
    };
    let bytes = encode_cmd(&probe)?;
    let back: RunnerCmd = serde_json::from_slice(&bytes[4..])?;
    assert!(matches!(
        back,
        RunnerCmd::ProbeEgress {
            target,
            anonymous_target,
            anonymous_egress: true,
            allow_loopback: true,
        } if target == "127.0.0.1:49999"
                && anonymous_target.as_deref() == Some("192.0.2.7:49999")
    ));
    // The off-loopback target and the two switches are optional on
    // the wire; the switches default to off.
    let back: RunnerCmd = serde_json::from_str(r#"{"ProbeEgress":{"target":"127.0.0.1:1"}}"#)?;
    assert!(matches!(
        back,
        RunnerCmd::ProbeEgress {
            anonymous_target: None,
            anonymous_egress: false,
            allow_loopback: false,
            ..
        }
    ));
    Ok(())
}
