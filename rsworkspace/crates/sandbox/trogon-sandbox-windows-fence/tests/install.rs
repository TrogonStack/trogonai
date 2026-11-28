use trogon_sandbox_windows_fence::{
    DEFAULT_PROXY_PORT_RANGE, DEFAULT_SANDBOX_USER, InstallContext, InstallOptions, ProxyPortRange, SandboxUser,
    SandboxUserStatus, SublayerGuid, install_args, install_command, parse_sandbox_status, profile_grant_notice,
    profile_grant_skip_reason,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn user(real: &str, session: Option<&str>) -> Result<SandboxUserStatus, Box<dyn std::error::Error>> {
    let json = format!(
        r#"{{"user":{{"user":{{"exists":true,"group_exists":true,"in_builtin_users":false,
        "in_sandbox_group":true,"hidden_from_logon":true}},"cred_present":true,
        "real_user_sid":"{real}"{}}},
        "wfp":{{"state":"installed","filters":1,"allow_loopback":false}}}}"#,
        session.map_or(String::new(), |s| format!(r#","session_user_sid":"{s}""#))
    );
    Ok(parse_sandbox_status(&json, &["status".to_string()])?.user)
}

#[test]
fn args_order_is_stable() -> TestResult {
    let opts = InstallOptions {
        sublayer_guid: Some(SublayerGuid::new("g")?),
        proxy_port_range: Some(ProxyPortRange::new(61000, 61009)?),
        allow_loopback: true,
        sandbox_user: Some(SandboxUser::new("u")?),
        force: true,
        ..InstallOptions::default()
    };
    assert_eq!(
        install_args(&opts),
        [
            "install",
            "--sublayer-guid",
            "g",
            "--proxy-port-range",
            "61000-61009",
            "--allow-loopback",
            "--sandbox-user",
            "u",
            "--force"
        ]
    );
    assert_eq!(install_args(&InstallOptions::default()), ["install"]);
    Ok(())
}

#[test]
fn suggested_command_names_only_non_defaults() -> TestResult {
    assert_eq!(
        install_command(&InstallContext::default(), false),
        "trogon-sandbox windows-install"
    );
    let ctx = InstallContext {
        sandbox_user: Some(SandboxUser::new(DEFAULT_SANDBOX_USER)?),
        proxy_port_range: Some(DEFAULT_PROXY_PORT_RANGE),
        ..InstallContext::default()
    };
    assert_eq!(install_command(&ctx, false), "trogon-sandbox windows-install");
    let ctx = InstallContext {
        sublayer_guid: Some(SublayerGuid::new("g")?),
        sandbox_user: Some(SandboxUser::new("other")?),
        allow_loopback: true,
        ..InstallContext::default()
    };
    assert_eq!(
        install_command(&ctx, true),
        "trogon-sandbox windows-install --sublayer-guid g --sandbox-user other --allow-loopback --force"
    );
    Ok(())
}

#[test]
fn suggested_command_names_a_custom_port_range() -> TestResult {
    let ctx = InstallContext {
        proxy_port_range: Some(ProxyPortRange::new(61000, 61009)?),
        ..InstallContext::default()
    };
    assert_eq!(
        install_command(&ctx, false),
        "trogon-sandbox windows-install --proxy-port-range 61000-61009"
    );
    Ok(())
}

#[test]
fn skip_reasons() -> TestResult {
    let me = "S-1-5-21-1-2-3-1000";
    assert_eq!(
        profile_grant_skip_reason(&user(me, Some(me))?, true).as_deref(),
        Some("--skip-profile-grant was set")
    );
    assert_eq!(profile_grant_skip_reason(&user(me, Some(me))?, false), None);
    assert_eq!(
        profile_grant_skip_reason(&user(me, Some(&me.to_lowercase()))?, false),
        None
    );
    let service = profile_grant_skip_reason(&user("S-1-5-18", Some("S-1-5-18"))?, false).ok_or("expected a reason")?;
    assert!(service.starts_with("the install ran as the service account S-1-5-18, which runs no sessions;"));
    let unknown = profile_grant_skip_reason(&user(me, None)?, false).ok_or("expected a reason")?;
    assert!(unknown.starts_with("the owner of the session this install ran in could not be determined"));
    let other = "S-1-5-21-1-2-3-1001";
    let foreign = profile_grant_skip_reason(&user(me, Some(other))?, false).ok_or("expected a reason")?;
    assert!(foreign.contains(&format!("in a session that belongs to {other}; granting would open")));
    Ok(())
}

#[test]
fn notice_names_the_profile_and_the_exits() {
    let n = profile_grant_notice("C:\\Users\\u");
    assert!(n.starts_with("Making C:\\Users\\u readable to sandboxed commands"));
    assert!(n.contains("--skip-profile-grant") && n.contains("fence-helper profile forget"));
}
