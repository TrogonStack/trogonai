use trogon_sandbox_windows_fence::{
    InstallContext, ProbeOutcome, ProxyPortRange, VerifyRun, WINDOWS_ALLOW_LOOPBACK_EFFECT, WfpResidual,
    WindowsSandboxErrorCode as Code, interpret_wfp_verify,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn run(status: i32, stdout: &str) -> VerifyRun {
    VerifyRun {
        status: Some(status),
        stdout: stdout.to_string(),
        stderr: "diag".to_string(),
    }
}

const CLEAN: &str = r#"{"egress_probe":"blocked","target":"127.0.0.1:50000",
  "anonymous_probe":"connected","anonymous_target":"10.0.0.5:50000",
  "anonymous_loopback_probe":"connected","deny_only_probe":"blocked",
  "low_integrity_probe":"blocked","low_integrity_loopback_probe":"blocked",
  "untrusted_integrity_probe":"blocked","untrusted_integrity_loopback_probe":"blocked",
  "allow_loopback":false,"residuals":["anonymous-egress","anonymous-loopback"]}"#;

#[test]
fn clean_pass_carries_residual_warnings() -> TestResult {
    let r = interpret_wfp_verify(&run(0, CLEAN), &InstallContext::default())?;
    assert_eq!(r.probes.plain, ProbeOutcome::Blocked);
    assert_eq!(
        r.residuals,
        [WfpResidual::AnonymousEgress, WfpResidual::AnonymousLoopback]
    );
    assert_eq!(r.warnings.len(), 2);
    assert!(r.warnings[0].starts_with("WFP egress fence residual 'anonymous-egress': "));
    assert_eq!(r.probes.open_loopback, None);
    Ok(())
}

#[test]
fn skipped_lowered_integrity_warns() -> TestResult {
    let json = CLEAN.replace(
        "\"low_integrity_probe\":\"blocked\"",
        "\"low_integrity_probe\":\"skipped\"",
    );
    let r = interpret_wfp_verify(&run(0, &json), &InstallContext::default())?;
    assert!(r.warnings.last().ok_or("no warning")?.contains("were skipped"));
    Ok(())
}

#[test]
fn open_loopback_adds_effect_text() -> TestResult {
    let json = CLEAN.replace(
        "\"allow_loopback\":false",
        "\"allow_loopback\":true,\"open_loopback_probe\":\"connected\"",
    );
    let r = interpret_wfp_verify(&run(0, &json), &InstallContext::default())?;
    assert_eq!(r.probes.open_loopback, Some(ProbeOutcome::Connected));
    assert_eq!(r.warnings.last().ok_or("no warning")?, WINDOWS_ALLOW_LOOPBACK_EFFECT);
    Ok(())
}

#[test]
fn open_loopback_unreachable_plain_is_inactive() -> TestResult {
    let json = CLEAN
        .replace("\"egress_probe\":\"blocked\"", "\"egress_probe\":\"unreachable\"")
        .replace(
            "\"allow_loopback\":false",
            "\"allow_loopback\":true,\"open_loopback_probe\":\"blocked\"",
        );
    let Err(e) = interpret_wfp_verify(&run(0, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpFenceInactive);
    assert!(e.message().contains("no answer where the filter must refuse at once"));
    Ok(())
}

#[test]
fn connected_attempt_rejects_even_on_exit_zero() -> TestResult {
    let json = CLEAN.replace("\"egress_probe\":\"blocked\"", "\"egress_probe\":\"connected\"");
    let Err(e) = interpret_wfp_verify(&run(0, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpFenceInactive);
    assert!(
        e.message()
            .contains("direct outbound from the sandbox user to 127.0.0.1:50000")
    );
    assert!(
        e.message()
            .contains("Re-run `trogon-sandbox windows-install` (one UAC prompt). (diag)")
    );
    Ok(())
}

#[test]
fn exit_three_without_probe_detail_is_inactive() -> TestResult {
    let Err(e) = interpret_wfp_verify(&run(3, CLEAN), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpFenceInactive);
    assert!(e.message().contains("a direct outbound connect was not blocked"));
    Ok(())
}

#[test]
fn loopback_mismatch_names_forced_reinstall() -> TestResult {
    let ctx = InstallContext {
        proxy_port_range: Some(ProxyPortRange::new(61000, 61009)?),
        ..InstallContext::default()
    };
    let json = r#"{"egress_probe":"blocked","target":"t","allow_loopback":true,"hint":"h"}"#;
    let Err(e) = interpret_wfp_verify(&run(19, json), &ctx) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpLoopbackMismatch);
    assert!(e.message().ends_with(
        "h. Through this package: `trogon-sandbox windows-install --proxy-port-range 61000-61009 \
         --allow-loopback --force`."
    ));
    Ok(())
}

#[test]
fn nonzero_without_leak_is_inconclusive() -> TestResult {
    let json = CLEAN.replace("\"egress_probe\":\"blocked\"", "\"egress_probe\":\"unreachable\"");
    let Err(e) = interpret_wfp_verify(&run(1, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpVerifyInconclusive);
    assert!(
        e.message()
            .contains("probe to 127.0.0.1:50000 was 'unreachable' (exit 1)")
    );
    Ok(())
}

#[test]
fn exit_zero_with_unblocked_plain_is_inconclusive() -> TestResult {
    let json = CLEAN.replace("\"egress_probe\":\"blocked\"", "\"egress_probe\":\"error\"");
    let Err(e) = interpret_wfp_verify(&run(0, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpVerifyInconclusive);
    assert!(e.message().contains("reported the plain attempt as 'error'"));
    Ok(())
}

#[test]
fn unparseable_output_is_typed() -> TestResult {
    let Err(e) = interpret_wfp_verify(&run(2, "garbage"), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpVerifyUnparseable);
    assert!(e.message().contains("exited 2 with unparseable output \"garbage\""));
    Ok(())
}

#[test]
fn an_outdated_helper_is_named() -> TestResult {
    let old = VerifyRun {
        status: Some(2),
        stdout: String::new(),
        stderr: "error: unexpected argument '--allow-loopback' found".to_string(),
    };
    let Err(e) = interpret_wfp_verify(&old, &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperTooOld);
    Ok(())
}

#[test]
fn a_probe_outcome_not_known_is_refused() -> TestResult {
    let json = CLEAN.replace("\"deny_only_probe\":\"blocked\"", "\"deny_only_probe\":\"odd\"");
    let Err(e) = interpret_wfp_verify(&run(0, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    assert!(e.message().contains("`deny_only_probe` is \"odd\""));
    Ok(())
}

#[test]
fn a_missing_probe_is_refused() -> TestResult {
    let json = CLEAN.replace("\"deny_only_probe\":\"blocked\",", "");
    let Err(e) = interpret_wfp_verify(&run(0, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    assert!(e.message().contains("no `deny_only_probe`"));
    Ok(())
}

#[test]
fn a_missing_open_loopback_probe_is_refused_when_loopback_is_open() -> TestResult {
    let json = CLEAN.replace("\"allow_loopback\":false", "\"allow_loopback\":true");
    let Err(e) = interpret_wfp_verify(&run(0, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    assert!(e.message().contains("no `open_loopback_probe`"));
    Ok(())
}

#[test]
fn a_missing_allow_loopback_is_refused() -> TestResult {
    let json = CLEAN.replace("\"allow_loopback\":false,", "");
    let Err(e) = interpret_wfp_verify(&run(0, &json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::FenceHelperBadJson);
    assert!(e.message().contains("does not say `allow_loopback`"));
    Ok(())
}

#[test]
fn every_outcome_displays() {
    let shown: Vec<String> = [ProbeOutcome::Refused, ProbeOutcome::Skipped]
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(shown, ["refused", "skipped"]);
}

#[test]
fn a_residual_this_release_does_not_know_is_still_reported() -> TestResult {
    let json = CLEAN.replace("\"anonymous-loopback\"]", "\"anonymous-loopback\",\"unknown-one\"]");
    let r = interpret_wfp_verify(&run(0, &json), &InstallContext::default())?;
    assert_eq!(
        r.residuals,
        [WfpResidual::AnonymousEgress, WfpResidual::AnonymousLoopback]
    );
    assert_eq!(r.warnings.len(), 3);
    assert!(r.warnings[2].contains("'unknown-one'") && r.warnings[2].contains("treat it as a gap"));
    Ok(())
}
