use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TOKEN: &str = concat!("lease", "proxy", "42");

fn environment(ca: Option<CaCertificate>) -> Result<ProxyEnvironment, Box<dyn std::error::Error>> {
    Ok(ProxyEnvironment::new(
        ProxyEndpoint::relay(),
        ProxyCredentials::new(LeaseProxyToken::parse(TOKEN)?),
        ca,
    ))
}

fn get<'a>(vars: &'a [(String, String)], name: &str) -> Option<&'a str> {
    vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

#[test]
fn proxy_variables_point_at_the_relay_with_the_token() -> TestResult {
    let vars = environment(None)?.vars();
    let expected = format!("http://sandbox:{TOKEN}@localhost:3128");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "GRPC_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "grpc_proxy",
    ] {
        assert_eq!(get(&vars, name), Some(expected.as_str()), "{name}");
    }
    Ok(())
}

#[test]
fn an_anonymous_proxy_gets_a_url_without_userinfo_and_tool_variables_without_credentials() -> TestResult {
    let jar = tempfile::NamedTempFile::new()?;
    let environment = ProxyEnvironment::new(ProxyEndpoint::relay(), ProxyCredentials::anonymous(), None)
        .with_tools(ToolProxyEnv::enabled(Some(jar.path().to_path_buf())));
    let vars = environment.vars();
    assert_eq!(get(&vars, "HTTPS_PROXY"), Some("http://localhost:3128"));
    assert_eq!(get(&vars, "FTP_PROXY"), Some("socks5h://localhost:1080"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_PORT"), Some("3128"));
    assert!(get(&vars, "CLOUDSDK_PROXY_PASSWORD").is_none());
    assert!(get(&vars, "GIT_CONFIG_PARAMETERS").is_none());
    Ok(())
}

#[test]
fn no_proxy_is_empty_unless_the_tool_variables_are_on() -> TestResult {
    let vars = environment(None)?.vars();
    assert_eq!(get(&vars, "NO_PROXY"), Some(""));
    assert_eq!(get(&vars, "no_proxy"), Some(""));
    Ok(())
}

#[test]
fn no_proxy_lists_loopback_and_private_ranges_as_the_tool_variables_do() -> TestResult {
    let vars = environment(None)?.with_tools(ToolProxyEnv::enabled(None)).vars();
    let list = "localhost,127.0.0.1,::1,169.254.0.0/16,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16";
    assert_eq!(get(&vars, "NO_PROXY"), Some(list));
    assert_eq!(get(&vars, "no_proxy"), Some(list));
    Ok(())
}

#[test]
fn the_host_ports_are_named_only_when_known() -> TestResult {
    let port = Port::new;
    assert!(get(&environment(None)?.vars(), "SANDBOX_HOST_HTTP_PROXY_PORT").is_none());
    let vars = environment(None)?
        .with_host_ports(HostProxyPorts::new(port(3128)?, port(1080)?))
        .vars();
    assert_eq!(get(&vars, "SANDBOX_HOST_HTTP_PROXY_PORT"), Some("3128"));
    assert_eq!(get(&vars, "SANDBOX_HOST_SOCKS_PROXY_PORT"), Some("1080"));
    Ok(())
}

#[test]
fn tool_variables_are_absent_unless_enabled() -> TestResult {
    let vars = environment(None)?.vars();
    for name in [
        "GIT_SSH_COMMAND",
        "FTP_PROXY",
        "RSYNC_PROXY",
        "DOCKER_HTTP_PROXY",
        "CLOUDSDK_PROXY_TYPE",
        "GIT_CONFIG_PARAMETERS",
    ] {
        assert!(get(&vars, name).is_none(), "{name}");
    }
    Ok(())
}

#[test]
fn enabled_tool_variables_point_at_the_relay() -> TestResult {
    let jar = tempfile::NamedTempFile::new()?;
    let tools = ToolProxyEnv::enabled(Some(jar.path().to_path_buf()));
    let vars = environment(None)?.with_tools(tools).vars();
    assert_eq!(
        get(&vars, "FTP_PROXY"),
        Some(format!("socks5h://sandbox:{TOKEN}@localhost:1080").as_str())
    );
    assert_eq!(
        get(&vars, "DOCKER_HTTPS_PROXY"),
        Some(format!("http://sandbox:{TOKEN}@localhost:3128").as_str())
    );
    assert!(
        get(&vars, "GIT_SSH_COMMAND")
            .ok_or("GIT_SSH_COMMAND is set")?
            .contains("PROXY:localhost:%h:%p,proxyport=3128")
    );
    assert_eq!(get(&vars, "RSYNC_PROXY"), Some("localhost:1080"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_TYPE"), Some("http"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_ADDRESS"), Some("localhost"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_PORT"), Some("3128"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_USERNAME"), Some("sandbox"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_PASSWORD"), Some(TOKEN));
    assert_eq!(
        get(&vars, "GIT_CONFIG_PARAMETERS"),
        Some("'http.proxyAuthMethod=basic'")
    );
    assert!(get(&vars, "JAVA_TOOL_OPTIONS").is_none());
    Ok(())
}

#[test]
fn java_options_carry_the_agent_and_never_the_token() -> TestResult {
    let jar = tempfile::NamedTempFile::new()?;
    let tools = ToolProxyEnv::enabled(Some(jar.path().to_path_buf()));
    let options = environment(None)?
        .with_tools(tools)
        .java_tool_options(Some("-Da=1"))
        .ok_or("the java options are set")?;
    assert_eq!(options, format!("-javaagent:{} -Da=1", jar.path().display()));
    assert!(!options.contains(TOKEN));
    Ok(())
}

#[test]
fn an_attributed_environment_names_the_command_in_the_proxy_user() -> TestResult {
    let vars = environment(None)?
        .with_attribution(&CommandAttribution::new("echo hi"))
        .vars();
    assert_eq!(
        get(&vars, "HTTPS_PROXY"),
        Some(format!("http://sandbox.ZWNobyBoaQ%3D%3D:{TOKEN}@localhost:3128").as_str())
    );
    Ok(())
}

#[test]
fn without_a_ca_no_trust_variable_is_set() -> TestResult {
    let environment = environment(None)?;
    assert!(get(&environment.vars(), "SSL_CERT_FILE").is_none());
    assert!(environment.ca_bind().is_none());
    Ok(())
}

#[test]
fn with_a_ca_every_trust_variable_points_at_the_guest_path() -> TestResult {
    let environment = environment(Some(CaCertificate::new("/host/ca.pem".into())))?;
    let vars = environment.vars();
    for name in trogon_sandbox_protocol::CA_TRUST_ENV_VARS {
        assert_eq!(get(&vars, name), Some(GUEST_CA_PATH), "{name}");
    }
    Ok(())
}

#[test]
fn the_ca_is_bound_read_only_at_the_guest_path() -> TestResult {
    let bind = environment(Some(CaCertificate::new("/host/ca.pem".into())))?
        .ca_bind()
        .ok_or("a ca bind")?;
    assert_eq!(bind.host_path, PathBuf::from("/host/ca.pem"));
    assert_eq!(bind.sandbox_path, GUEST_CA_PATH);
    assert!(bind.readonly);
    Ok(())
}

#[test]
fn debug_output_never_shows_the_token() -> TestResult {
    let environment = environment(Some(CaCertificate::new("/host/ca.pem".into())))?;
    assert!(!format!("{environment:?}").contains(TOKEN));
    assert!(!format!("{:?}", environment.credentials).contains(TOKEN));
    Ok(())
}
