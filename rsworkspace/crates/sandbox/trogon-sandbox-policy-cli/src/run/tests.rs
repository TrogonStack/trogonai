use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_config_carries_credentials_and_the_cli_network_extras() -> TestResult {
    let settings = SandboxSettings::from_json_lenient(
        r#"{"network": {"allowedDomains": ["example.com"], "deniedDomains": [], "tlsTerminate": {}},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
            "credentials": {"envVars": [{"name": "MASKED_TOKEN", "mode": "mask"}]}}"#,
        None,
    )?;
    let config = config_of(Path::new("/workspace"), &settings)?;
    assert_eq!(config.bindings().len(), 1);
    assert!(config.configured_network_options().private_ranges_by_name.is_enabled());
    Ok(())
}
