use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TOKEN: &str = concat!("lease", "proxy", "42");

fn environment() -> Result<ProxyEnvironment, Box<dyn std::error::Error>> {
    Ok(ProxyEnvironment::new(
        ProxyEndpoint::loopback("127.0.0.1:51234".parse()?),
        ProxyCredentials::new(LeaseProxyToken::parse(TOKEN)?),
        None,
    ))
}

fn get<'a>(vars: &'a [(String, String)], name: &str) -> Option<&'a str> {
    vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

#[test]
fn the_proxy_is_spelled_localhost_with_the_port_of_the_listener() -> TestResult {
    let vars = environment()?.vars();
    let expected = format!("http://sandbox:{TOKEN}@localhost:51234");
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "GRPC_PROXY"] {
        assert_eq!(get(&vars, name), Some(expected.as_str()), "{name}");
        assert_eq!(get(&vars, &name.to_lowercase()), Some(expected.as_str()), "{name}");
    }
    Ok(())
}

#[test]
fn an_attributed_command_puts_its_base64_key_in_the_user_name() -> TestResult {
    let vars = environment()?
        .with_attribution(&CommandAttribution::new("echo hi"))
        .vars();
    assert_eq!(
        get(&vars, "HTTPS_PROXY"),
        Some(format!("http://sandbox.ZWNobyBoaQ%3D%3D:{TOKEN}@localhost:51234").as_str())
    );
    Ok(())
}

#[test]
fn the_tool_variables_use_the_attributed_user_and_localhost() -> TestResult {
    let vars = environment()?
        .with_attribution(&CommandAttribution::new("echo hi"))
        .with_tools(ToolProxyEnv::enabled(None))
        .vars();
    assert_eq!(get(&vars, "RSYNC_PROXY"), Some("localhost:51234"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_ADDRESS"), Some("localhost"));
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_USERNAME"), Some("sandbox.ZWNobyBoaQ=="));
    assert_eq!(
        get(&vars, "FTP_PROXY"),
        Some(format!("socks5h://sandbox.ZWNobyBoaQ%3D%3D:{TOKEN}@localhost:51234").as_str())
    );
    assert!(
        get(&vars, "GIT_SSH_COMMAND")
            .ok_or("a git ssh command")?
            .contains("ProxyCommand=")
    );
    Ok(())
}

#[test]
fn a_jvm_is_told_to_use_the_ipv4_stack_even_without_an_agent() -> TestResult {
    assert_eq!(
        environment()?.java_tool_options(None).as_deref(),
        Some(PREFER_IPV4_STACK)
    );
    Ok(())
}

#[test]
fn the_payload_java_options_sit_between_the_agent_and_the_ipv4_flag() -> TestResult {
    let jar = tempfile::NamedTempFile::new()?;
    let options = environment()?
        .with_tools(ToolProxyEnv::enabled(Some(jar.path().to_path_buf())))
        .java_tool_options(Some("-Da=1"))
        .ok_or("java options")?;
    assert_eq!(
        options,
        format!("-javaagent:{} -Da=1 {PREFER_IPV4_STACK}", jar.path().display())
    );
    Ok(())
}

#[test]
fn the_ipv4_flag_is_not_repeated_when_the_payload_has_it() -> TestResult {
    let inherited = format!("-Da=1 {PREFER_IPV4_STACK}");
    assert_eq!(environment()?.java_tool_options(Some(&inherited)), Some(inherited));

    Ok(())
}
