use trogon_sandbox_windows_fence::{
    CAPABILITIES_WHY, DependencyCheck, InstallContext, REQUIRED_CAPABILITIES, SandboxStatus,
    WINDOWS_ALLOW_LOOPBACK_EFFECT, WindowsSandboxErrorCode as Code, check_capabilities, check_dependencies,
    fence_helper_outdated_error, missing_capability, parse_sandbox_status,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn status(user_extra: &str, wfp: &str) -> Result<SandboxStatus, Box<dyn std::error::Error>> {
    let json = format!(
        r#"{{"user":{{"user":{{"exists":true,"sid":"S-1","group_exists":true,
        "in_builtin_users":false,"in_sandbox_group":true,"hidden_from_logon":true}},
        "cred_present":true,"real_user_sid":"S-2"{user_extra}}},
        "wfp":{wfp}}}"#
    );
    Ok(parse_sandbox_status(&json, &[])?)
}

const INSTALLED: &str = r#"{"state":"installed","filters":6,"port_range":[60080,60089],"allow_loopback":false}"#;

fn check(status: &SandboxStatus, allow_loopback: bool) -> DependencyCheck {
    check_dependencies(
        &status.user,
        &status.wfp,
        &InstallContext {
            allow_loopback,
            ..InstallContext::default()
        },
    )
}

#[test]
fn a_matching_install_has_no_error() -> TestResult {
    let s = status("", INSTALLED)?;
    let c = check(&s, false);
    assert!(c.errors.is_empty() && c.warnings.is_empty());
    assert!(c.error().is_none());
    Ok(())
}

#[test]
fn another_setup_version_asks_for_a_reinstall() -> TestResult {
    let s = status(r#","marker_version":3,"setup_version":4"#, INSTALLED)?;
    let error = check(&s, false).error().ok_or("expected an error")?;
    assert_eq!(error.code(), Code::ReinstallRequired);
    assert!(error.message().contains("different release"));
    let newer = status(r#","marker_version":5,"setup_version":4"#, INSTALLED)?;
    let error = check(&newer, false).error().ok_or("expected an error")?;
    assert!(error.message().contains("newer release"));
    let unjudged = status(r#","marker_version":3"#, INSTALLED)?;
    assert!(check(&unjudged, false).error().is_none());
    Ok(())
}

#[test]
fn a_loopback_mode_other_than_the_configured_one_is_a_mismatch_either_way() -> TestResult {
    let closed = status("", INSTALLED)?;
    let error = check(&closed, true).error().ok_or("expected an error")?;
    assert_eq!(error.code(), Code::WfpLoopbackMismatch);
    assert!(error.message().contains("--allow-loopback --force"));
    let open = status(
        "",
        r#"{"state":"installed","filters":6,"port_range":[60080,60089],"allow_loopback":true}"#,
    )?;
    let error = check(&open, false).error().ok_or("expected an error")?;
    assert_eq!(error.code(), Code::WfpLoopbackMismatch);
    let agreed = check(&open, true);
    assert!(agreed.error().is_none());
    assert_eq!(agreed.warnings, [WINDOWS_ALLOW_LOOPBACK_EFFECT]);
    Ok(())
}

#[test]
fn a_missing_secondary_logon_service_blocks() -> TestResult {
    let s = status(r#","seclogon":"disabled""#, INSTALLED)?;
    let error = check(&s, false).error().ok_or("expected an error")?;
    assert_eq!(error.code(), Code::SeclogonDisabled);
    let missing = status(r#","seclogon":"missing""#, INSTALLED)?;
    let error = check(&missing, false).error().ok_or("expected an error")?;
    assert_eq!(error.code(), Code::SeclogonDisabled);
    let unreadable = status(r#","seclogon":"cannot-read""#, INSTALLED)?;
    assert!(check(&unreadable, false).error().is_none());
    Ok(())
}

#[test]
fn absent_filters_ask_for_the_install() -> TestResult {
    let s = status("", r#"{"state":"absent","filters":0,"allow_loopback":false}"#)?;
    let error = check(&s, false).error().ok_or("expected an error")?;
    assert!(error.message().contains("WFP filters not installed"));
    let unreadable = status("", r#"{"state":"cannot-read","filters":0,"allow_loopback":false}"#)?;
    assert!(check(&unreadable, false).error().is_none());
    Ok(())
}

#[test]
fn capabilities_are_asked_for_by_name() {
    let all: Vec<String> = REQUIRED_CAPABILITIES.iter().map(|c| c.to_string()).collect();
    assert_eq!(missing_capability(&all), None);
    assert_eq!(missing_capability(&all[1..]), Some("profile-deny-expansion"));
    let error = fence_helper_outdated_error("exec-holder", CAPABILITIES_WHY);
    assert_eq!(error.code(), Code::FenceHelperOutdated);
    assert!(error.message().contains("'exec-holder'"));
}

#[test]
fn an_unprovisioned_account_blocks_a_session() -> TestResult {
    let s = parse_sandbox_status(
        r#"{"user":{"user":{"exists":false,"group_exists":false,"in_builtin_users":false,
        "in_sandbox_group":false,"hidden_from_logon":false},"cred_present":false,"real_user_sid":"S-2"},
        "wfp":{"state":"absent","filters":0,"allow_loopback":false}}"#,
        &[],
    )?;
    let error = check(&s, false).error().ok_or("expected an error")?;
    assert_eq!(error.code(), Code::NotProvisioned);
    assert!(error.message().contains("not provisioned"));
    Ok(())
}

#[test]
fn filters_whose_loopback_mode_is_not_the_recorded_one_are_reported() -> TestResult {
    let s = status("", r#"{"state":"loopback-mismatch","filters":6,"allow_loopback":true}"#)?;
    let error = check(&s, false).error().ok_or("expected an error")?;
    assert_eq!(error.code(), Code::WfpLoopbackMismatch);
    assert!(error.message().contains("(open)"));
    Ok(())
}

#[test]
fn capabilities_the_helper_lacks_are_refused() -> TestResult {
    let s = status("", INSTALLED)?;
    let outdated = check_capabilities(&s.user);
    if missing_capability(&s.user.capabilities).is_some() {
        let error = outdated.err().ok_or("expected an error")?;
        assert_eq!(error.code(), Code::FenceHelperOutdated);
    }
    let mut full = s.user.clone();
    full.capabilities = REQUIRED_CAPABILITIES.iter().map(|c| c.to_string()).collect();
    check_capabilities(&full)?;
    Ok(())
}

#[test]
fn a_matching_setup_version_is_not_a_mismatch() -> TestResult {
    let s = status(r#","marker_version":4,"setup_version":4"#, INSTALLED)?;
    assert!(trogon_sandbox_windows_fence::setup_version_mismatch(&s.user, None).is_none());
    Ok(())
}
