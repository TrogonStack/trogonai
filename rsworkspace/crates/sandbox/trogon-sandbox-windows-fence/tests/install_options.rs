use trogon_sandbox_windows_fence::{InstallOptions, ProxyPortRange, SandboxUser, SublayerGuid};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_context_carries_what_identifies_the_install() -> TestResult {
    let options = InstallOptions {
        sublayer_guid: Some(SublayerGuid::new("{guid}")?),
        proxy_port_range: Some(ProxyPortRange::new(60100, 60105)?),
        allow_loopback: true,
        sandbox_user: Some(SandboxUser::new("fence-user")?),
        force: true,
        ..InstallOptions::default()
    };
    let context = options.context();
    assert_eq!(context.sublayer_guid, options.sublayer_guid);
    assert_eq!(context.proxy_port_range, options.proxy_port_range);
    assert_eq!(context.sandbox_user, options.sandbox_user);
    assert!(context.allow_loopback);
    Ok(())
}
