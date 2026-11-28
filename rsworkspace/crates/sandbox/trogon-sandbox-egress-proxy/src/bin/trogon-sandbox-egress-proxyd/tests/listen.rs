//! The listen flags and the refusal of a non-loopback listen address.

use super::*;

fn config_with(pairs: &[(&str, &str)]) -> Result<Config, String> {
    let mut env = vec![(POLICY_FILE_ENV_VAR, "/policy.json")];
    env.extend_from_slice(pairs);
    load_config(env_map(&env), file_map(&[("/policy.json", r#""deny_all""#)]))
}

fn token_env(key: &str) -> Option<String> {
    (key == MUX_TOKEN_ENV_VAR).then(|| "token".to_string())
}

#[test]
fn the_default_listen_address_is_loopback() -> TestResult {
    assert!(is_loopback_listen(DEFAULT_LISTEN_ADDR));
    config_with(&[])?.require_loopback_listeners()?;
    Ok(())
}

#[test]
fn listen_flags_parse_and_reject_unknown_arguments() -> TestResult {
    assert_eq!(flags(&[])?, ListenFlags::default());
    let parsed = flags(&["--listen", "127.0.0.1:8080", "--allow-non-loopback-listen"])?;
    assert_eq!(parsed.listen.as_deref(), Some("127.0.0.1:8080"));
    assert!(parsed.allow_non_loopback_listen);
    assert_eq!(flags(&["--listen-fd", "7"])?.listen_fd, Some(7));
    assert!(flags(&["--listen-fd", "1"]).is_err());
    assert!(flags(&["--listen-fd", "x"]).is_err());
    assert!(flags(&["--listen"]).is_err());
    assert!(flags(&["--nope"]).is_err());
    assert!(flags(&["--listen-any-interface"]).is_err());
    assert!(flags(&["--listen-fd", "7", "--listen", "127.0.0.1:1"]).is_err());
    Ok(())
}

#[test]
fn a_non_loopback_flag_address_is_refused_unless_the_flag_opts_in() -> TestResult {
    for addr in ["0.0.0.0:3128", "192.168.1.5:3128", "[::]:3128", "example.com:80"] {
        let applied = flags(&["--listen", addr])?.apply(base_config()?, token_env)?;
        let err = applied.require_loopback_listeners().err().ok_or("expected an error")?;
        assert!(
            err.contains(addr) && err.contains("--allow-non-loopback-listen"),
            "{err}"
        );

        let allowed = flags(&["--listen", addr, "--allow-non-loopback-listen"])?.apply(base_config()?, token_env)?;
        allowed.require_loopback_listeners()?;
        assert_eq!(
            allowed.mux.ok_or("expected a value")?.listen_addr,
            ListenAddress::Tcp(addr.to_string())
        );
    }
    for addr in ["127.0.0.1:0", "127.9.9.9:80", "[::1]:80", "localhost:80"] {
        flags(&["--listen", addr])?
            .apply(base_config()?, token_env)?
            .require_loopback_listeners()?;
    }
    Ok(())
}

#[test]
fn a_non_loopback_transparent_address_is_refused_unless_the_environment_opts_in() -> TestResult {
    let refused = config_with(&[(LISTEN_ADDR_ENV_VAR, "0.0.0.0:3128")])?
        .require_loopback_listeners()
        .err()
        .ok_or("expected an error")?;
    assert!(refused.contains(LISTEN_ADDR_ENV_VAR) && refused.contains(ALLOW_NON_LOOPBACK_LISTEN_ENV_VAR));

    config_with(&[
        (LISTEN_ADDR_ENV_VAR, "0.0.0.0:3128"),
        (ALLOW_NON_LOOPBACK_LISTEN_ENV_VAR, "true"),
    ])?
    .require_loopback_listeners()?;
    let flag_opt_in = flags(&["--allow-non-loopback-listen"])?
        .apply(config_with(&[(LISTEN_ADDR_ENV_VAR, "0.0.0.0:3128")])?, |_| None)?;
    assert!(flag_opt_in.mux.is_none());
    flag_opt_in.require_loopback_listeners()?;
    Ok(())
}

#[test]
fn a_non_loopback_explicit_proxy_address_from_the_environment_is_refused_unless_opted_in() -> TestResult {
    let mux = [
        (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
        (MUX_LISTEN_ADDR_ENV_VAR, "10.1.2.3:3128"),
        (MUX_TOKEN_ENV_VAR, "token"),
    ];
    let refused = config_with(&mux)?
        .require_loopback_listeners()
        .err()
        .ok_or("expected an error")?;
    assert!(refused.contains(MUX_LISTEN_ADDR_ENV_VAR), "{refused}");

    let mut opted_in = mux.to_vec();
    opted_in.push((ALLOW_NON_LOOPBACK_LISTEN_ENV_VAR, "true"));
    config_with(&opted_in)?.require_loopback_listeners()?;
    Ok(())
}

#[test]
fn unix_sockets_and_inherited_descriptors_need_no_opt_in() -> TestResult {
    config_with(&[
        (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
        (MUX_LISTEN_ADDR_ENV_VAR, "unix:/run/p.sock"),
        (MUX_TOKEN_ENV_VAR, "token"),
    ])?
    .require_loopback_listeners()?;
    flags(&["--listen-fd", "9"])?
        .apply(base_config()?, |_| None)?
        .require_loopback_listeners()?;
    Ok(())
}

#[test]
fn the_opt_in_value_must_be_true_or_false() -> TestResult {
    let err = config_with(&[(ALLOW_NON_LOOPBACK_LISTEN_ENV_VAR, "yes")])
        .err()
        .ok_or("expected an error")?;
    assert!(err.contains(ALLOW_NON_LOOPBACK_LISTEN_ENV_VAR), "{err}");
    assert!(!config_with(&[(ALLOW_NON_LOOPBACK_LISTEN_ENV_VAR, "false")])?.allow_non_loopback_listen);
    Ok(())
}

#[test]
fn listen_fd_adopts_an_inherited_socket() -> TestResult {
    let config = flags(&["--listen-fd", "9"])?.apply(base_config()?, |_| None)?;
    assert_eq!(config.mux.ok_or("expected a value")?.inherited_fd, Some(9));
    Ok(())
}

#[test]
fn listen_flags_without_any_proxy_token_are_refused() -> TestResult {
    let config = config_with(&[])?;
    let err = flags(&["--listen", "127.0.0.1:0"])?
        .apply(config.clone(), |_| None)
        .err()
        .ok_or("expected an error")?;
    assert!(err.contains(MUX_TOKEN_ENV_VAR), "{err}");
    let with_token = flags(&["--listen", "127.0.0.1:0"])?.apply(config, token_env)?;
    assert!(with_token.mux.is_some());
    Ok(())
}

#[test]
fn empty_listen_flags_leave_the_config_untouched() -> TestResult {
    let config = base_config()?;
    let applied = ListenFlags::default().apply(config.clone(), |_| None)?;
    assert_eq!(applied, config);
    Ok(())
}

#[test]
fn a_listen_address_without_a_port_is_checked_as_a_host() {
    assert!(is_loopback_listen("localhost"));
    assert!(is_loopback_listen("127.0.0.1"));
    assert!(is_loopback_listen("[::1]:80"));
    assert!(!is_loopback_listen("example.com"));
}
