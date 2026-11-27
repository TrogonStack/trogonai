use super::*;
use crate::attribution::CommandAttribution;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TOKEN: &str = concat!("lease", "proxy", "42");

fn addr() -> Result<SocketAddr, std::net::AddrParseError> {
    "127.0.0.1:3128".parse()
}

fn socks_addr() -> Result<SocketAddr, std::net::AddrParseError> {
    "127.0.0.1:1080".parse()
}

fn login() -> ProxyLogin<'static> {
    ProxyLogin {
        user: ProxyUser::bare(),
        token: TOKEN,
    }
}

fn vars(ssh: SshTransport) -> Result<Vec<(String, String)>, std::net::AddrParseError> {
    Ok(tool_vars(
        ProxyHost::Localhost,
        addr()?,
        socks_addr()?,
        Some(&login()),
        ssh,
    ))
}

fn get<'a>(vars: &'a [(String, String)], name: &str) -> Option<&'a str> {
    vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

#[test]
fn an_unavailable_ssh_transport_sets_no_ssh_command() -> TestResult {
    let vars = vars(SshTransport::Unavailable)?;
    assert!(get(&vars, "GIT_SSH_COMMAND").is_none());
    assert!(get(&vars, SSH_PROXY_SCRIPT_VAR).is_none());
    assert!(get(&vars, "HTTPS_PROXY").is_some() || get(&vars, "FTP_PROXY").is_some());
    Ok(())
}

#[test]
fn git_over_ssh_uses_socat_connect_with_the_credential() -> TestResult {
    let vars = vars(SshTransport::Socat)?;
    let expected = format!(
        "ssh -o ControlMaster=no -o ControlPath=none -o ProxyCommand='socat - PROXY:localhost:%h:%p,proxyport=3128,proxyauth=sandbox:{TOKEN}'"
    );
    assert_eq!(get(&vars, "GIT_SSH_COMMAND"), Some(expected.as_str()));
    assert!(get(&vars, SSH_PROXY_SCRIPT_VAR).is_none());
    Ok(())
}

#[test]
fn git_over_ssh_with_bsd_nc_keeps_the_script_and_token_out_of_the_command() -> TestResult {
    let vars = vars(SshTransport::BsdNc)?;
    let command = get(&vars, "GIT_SSH_COMMAND").ok_or("a variable")?;
    assert_eq!(
        command,
        r#"ssh -o ControlMaster=no -o ControlPath=none -o ProxyCommand="/bin/sh -c 'eval \"\${SANDBOX_SSH_PROXY_COMMAND:?unset, but GIT_SSH_COMMAND needs it}\"' sh '%h' %p""#
    );
    assert!(!command.contains(TOKEN));
    let basic = STANDARD.encode(format!("sandbox:{TOKEN}"));
    let script = get(&vars, SSH_PROXY_SCRIPT_VAR).ok_or("a variable")?;
    assert!(script.contains(&format!("Proxy-Authorization: Basic {basic}")));
    assert!(script.contains("/usr/bin/nc 127.0.0.1 1080 | "));
    Ok(())
}

#[test]
fn rsync_and_gcloud_point_at_the_proxy_and_git_pre_sends_basic() -> TestResult {
    let vars = vars(SshTransport::Socat)?;
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
    Ok(())
}

#[test]
fn an_attributed_user_is_percent_encoded_in_urls_and_raw_elsewhere() -> TestResult {
    let login = ProxyLogin {
        user: CommandAttribution::new("echo hi").proxy_user(),
        token: TOKEN,
    };
    let socat = tool_vars(
        ProxyHost::Localhost,
        addr()?,
        socks_addr()?,
        Some(&login),
        SshTransport::Socat,
    );
    let bsd = tool_vars(
        ProxyHost::Localhost,
        addr()?,
        socks_addr()?,
        Some(&login),
        SshTransport::BsdNc,
    );
    assert_eq!(
        get(&socat, "DOCKER_HTTP_PROXY"),
        Some(format!("http://sandbox.ZWNobyBoaQ%3D%3D:{TOKEN}@localhost:3128").as_str())
    );
    assert_eq!(get(&socat, "CLOUDSDK_PROXY_USERNAME"), Some("sandbox.ZWNobyBoaQ=="));
    assert!(
        get(&socat, "GIT_SSH_COMMAND")
            .ok_or("a git ssh command")?
            .contains(&format!("proxyauth=sandbox.ZWNobyBoaQ==:{TOKEN}"))
    );
    let basic = STANDARD.encode(format!("sandbox.ZWNobyBoaQ==:{TOKEN}"));
    assert!(
        get(&bsd, SSH_PROXY_SCRIPT_VAR)
            .ok_or("a proxy script")?
            .contains(&format!("Basic {basic}"))
    );
    Ok(())
}

#[test]
fn without_a_login_no_credential_variable_is_set() -> TestResult {
    let vars = tool_vars(ProxyHost::Localhost, addr()?, socks_addr()?, None, SshTransport::Socat);
    for name in [
        "CLOUDSDK_PROXY_USERNAME",
        "CLOUDSDK_PROXY_PASSWORD",
        "GIT_CONFIG_PARAMETERS",
    ] {
        assert!(get(&vars, name).is_none(), "{name}");
    }
    assert_eq!(get(&vars, "CLOUDSDK_PROXY_PORT"), Some("3128"));
    assert_eq!(get(&vars, "DOCKER_HTTP_PROXY"), Some("http://localhost:3128"));
    assert_eq!(
        get(&vars, "GIT_SSH_COMMAND"),
        Some(
            "ssh -o ControlMaster=no -o ControlPath=none -o ProxyCommand='socat - PROXY:localhost:%h:%p,proxyport=3128'"
        )
    );
    Ok(())
}

#[test]
fn an_anonymous_bsd_nc_ssh_asks_the_socks_listener() -> TestResult {
    let vars = tool_vars(ProxyHost::Localhost, addr()?, socks_addr()?, None, SshTransport::BsdNc);
    assert_eq!(
        get(&vars, "GIT_SSH_COMMAND"),
        Some("ssh -o ControlMaster=no -o ControlPath=none -o ProxyCommand='nc -X 5 -x localhost:1080 %h %p'")
    );
    assert!(get(&vars, SSH_PROXY_SCRIPT_VAR).is_none());
    Ok(())
}

#[test]
fn ftp_goes_through_socks_with_remote_dns() -> TestResult {
    let vars = vars(SshTransport::Socat)?;
    let expected = format!("socks5h://sandbox:{TOKEN}@localhost:1080");
    assert_eq!(get(&vars, "FTP_PROXY"), Some(expected.as_str()));
    assert_eq!(get(&vars, "ftp_proxy"), Some(expected.as_str()));
    Ok(())
}

#[test]
fn docker_uses_the_http_proxy_for_both_schemes() -> TestResult {
    let vars = vars(SshTransport::Socat)?;
    let expected = format!("http://sandbox:{TOKEN}@localhost:3128");
    assert_eq!(get(&vars, "DOCKER_HTTP_PROXY"), Some(expected.as_str()));
    assert_eq!(get(&vars, "DOCKER_HTTPS_PROXY"), Some(expected.as_str()));
    Ok(())
}

#[test]
fn without_an_agent_jar_java_has_no_agent() -> TestResult {
    assert!(JavaProxyAgent::new("/no/such/agent.jar".into()).is_none());
    assert!(ToolProxyEnv::enabled(None).java_agent().is_none());
    assert_eq!(java_tool_options(None, &[], None), None);
    Ok(())
}

#[test]
fn a_jar_path_with_whitespace_is_quoted() -> TestResult {
    let dir = tempfile::tempdir()?;
    let jar = dir.path().join("with space.jar");
    std::fs::write(&jar, b"")?;
    let agent = JavaProxyAgent::new(jar.clone()).ok_or("a java agent")?;
    assert_eq!(agent.flag(), format!("\"-javaagent:{}\"", jar.display()));
    Ok(())
}

#[test]
fn java_options_put_the_agent_first_then_the_payload_then_new_flags() -> TestResult {
    let jar = tempfile::NamedTempFile::new()?;
    let agent = JavaProxyAgent::new(jar.path().to_path_buf()).ok_or("a java agent")?;
    assert_eq!(
        java_tool_options(Some(&agent), &[PREFER_IPV4_STACK], Some("-Dhttps.proxyPort=9")),
        Some(format!(
            "-javaagent:{} -Dhttps.proxyPort=9 {PREFER_IPV4_STACK}",
            jar.path().display()
        ))
    );
    Ok(())
}

#[test]
fn java_options_do_not_repeat_a_flag_the_payload_has() -> TestResult {
    let inherited = format!("-Da=1 {PREFER_IPV4_STACK}");
    assert_eq!(
        java_tool_options(None, &[PREFER_IPV4_STACK], Some(&inherited)),
        Some(inherited)
    );
    assert_eq!(
        java_tool_options(None, &[PREFER_IPV4_STACK], None),
        Some(PREFER_IPV4_STACK.to_string())
    );
    assert_eq!(java_tool_options(None, &[], Some("")), None);
    Ok(())
}

#[test]
fn the_inherited_options_are_kept_verbatim() -> TestResult {
    assert_eq!(
        java_tool_options(None, &[], Some(" -Da=1 ")),
        Some(" -Da=1 ".to_string())
    );
    Ok(())
}

#[test]
fn tool_variables_are_off_by_default() -> TestResult {
    assert!(!ToolProxyEnv::default().is_enabled());
    assert!(ToolProxyEnv::enabled(None).is_enabled());
    Ok(())
}
