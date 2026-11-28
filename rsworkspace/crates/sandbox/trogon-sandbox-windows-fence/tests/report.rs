use trogon_sandbox_windows_fence::{
    InstallProfileGrant, ProfileGrantItem, ProfileGrantResult, SandboxStatus, WINDOWS_ALLOW_LOOPBACK_EFFECT,
    parse_sandbox_status, render_install_report,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn status(loopback: bool) -> Result<SandboxStatus, Box<dyn std::error::Error>> {
    let json = format!(
        r#"{{"user":{{"user":{{"exists":true,"sid":"S-1-5-21-9","group_exists":true,
        "in_builtin_users":false,"in_sandbox_group":true,"hidden_from_logon":true}},
        "cred_present":true,"real_user_sid":"S-1"}},
        "wfp":{{"state":"installed","filters":6,"port_range":[60080,60089],
        "allow_loopback":{loopback}}}}}"#
    );
    Ok(parse_sandbox_status(&json, &["status".to_string()])?)
}

fn applied() -> InstallProfileGrant {
    InstallProfileGrant::Applied {
        user_sid: "S-1".to_string(),
        result: ProfileGrantResult {
            profile: "C:\\Users\\u".to_string(),
            containers: 0,
            granted: 8,
            files: 0,
            denied: 3,
            placeholders: 0,
            writes: 0,
            unchanged: 0,
            removed: 0,
            skipped: vec![ProfileGrantItem::default()],
            not_granted: vec![],
            self_closed: vec![],
            unverifiable: vec![],
            unprotected: vec![],
            stale_allow: vec![],
            forgotten_roots: vec![],
            elapsed_ms: 0,
        },
    }
}

#[test]
fn applied_report_text_is_stable() -> TestResult {
    assert_eq!(
        render_install_report(&status(false)?, &applied()),
        "Installed.\n  sandbox user: provisioned (S-1-5-21-9)\n  WFP:   installed, 6 filters, \
         port range 60080-60089\n  profile read grant: applied to C:\\Users\\u (8 directories, \
         3 excluded, 1 skipped)\n\nNo logout needed - the WFP filter keys on the dedicated \
         `trogon-sandbox` user's SID, so your network is unaffected."
    );
    Ok(())
}

#[test]
fn skipped_grant_and_open_loopback() -> TestResult {
    let out = render_install_report(
        &status(true)?,
        &InstallProfileGrant::Skipped {
            reason: "--skip-profile-grant was set".to_string(),
        },
    );
    assert!(out.contains("profile read grant: NOT applied - --skip-profile-grant was set\n\nNo logout"));
    assert!(out.ends_with(&format!("unaffected.\n\n{WINDOWS_ALLOW_LOOPBACK_EFFECT}")));
    Ok(())
}

#[test]
fn a_missing_user_and_absent_filters_are_reported() -> TestResult {
    let json = r#"{"user":{"user":{"exists":false,"group_exists":false,"in_builtin_users":false,
        "in_sandbox_group":false,"hidden_from_logon":false},"cred_present":false,"real_user_sid":"S-1"},
        "wfp":{"state":"absent","filters":0,"allow_loopback":false}}"#;
    let s = parse_sandbox_status(json, &[])?;
    let out = render_install_report(&s, &applied());
    assert!(out.starts_with("Installed.\n  sandbox user: MISSING\n  WFP:   absent, 0 filters\n"));
    Ok(())
}
