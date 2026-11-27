use super::*;

#[test]
fn both_outcomes_parse() -> Result<(), Box<dyn std::error::Error>> {
    let digest = "ab".repeat(32);
    let authenticated: TlsProbeResult = serde_json::from_str(&format!(
        r#"{{"outcome":"authenticated","certificate_sha256":"{digest}"}}"#
    ))?;
    assert_eq!(
        authenticated,
        TlsProbeResult::Authenticated {
            certificate_sha256: PeerCertificate::try_from(digest)?
        }
    );
    let blocked: TlsProbeResult = serde_json::from_str(r#"{"outcome":"blocked"}"#)?;
    assert_eq!(blocked, TlsProbeResult::Blocked {});
    Ok(())
}

#[test]
fn extra_unknown_or_invalid_diagnostics_are_refused() {
    for body in [
        r#"{"outcome":"blocked","unexpected":true}"#,
        r#"{"outcome":"delivered"}"#,
        r#"{"outcome":"authenticated","certificate_sha256":"nothex"}"#,
        r#"{"outcome":"authenticated"}"#,
        r#"{"outcome":"blocked""#,
    ] {
        assert!(serde_json::from_str::<TlsProbeResult>(body).is_err(), "{body}");
    }
}
