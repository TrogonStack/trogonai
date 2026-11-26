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
  "residuals":["anonymous-egress","anonymous-loopback","unknown-one"]}"#;

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
        "\"residuals\"",
        "\"allow_loopback\":true,\"open_loopback_probe\":\"connected\",\"residuals\"",
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
        .replace("\"residuals\"", "\"allow_loopback\":true,\"residuals\"");
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
    let json = r#"{"egress_probe":"unreachable","target":"t"}"#;
    let Err(e) = interpret_wfp_verify(&run(1, json), &InstallContext::default()) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::WfpVerifyInconclusive);
    assert!(e.message().contains("probe to t was 'unreachable' (exit 1)"));
    Ok(())
}

#[test]
fn exit_zero_with_unblocked_plain_is_inconclusive() -> TestResult {
    let json = r#"{"egress_probe":"error","target":"t"}"#;
    let Err(e) = interpret_wfp_verify(&run(0, json), &InstallContext::default()) else {
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
