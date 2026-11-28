use super::*;
#[cfg(unix)]
use std::io::Write as _;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::fd::IntoRawFd as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use trogon_sandbox_contract::DomainRule;
use trogon_sandbox_egress_proxy::credential_source::UnconfiguredCredentialSource;
use trogon_sandbox_egress_proxy::proxy::{TlsInterceptContext, handle_connection};
use trogon_sandbox_egress_proxy::resolve::VettingResolver;
use trogon_sandbox_egress_proxy::server::{SharedPolicy, intercept_domains_outside_ca, system_upstream_tls_config};
use trogon_sandbox_egress_proxy::tls_ca::PermittedDomains;

type TestError = Box<dyn std::error::Error>;
type TestResult = Result<(), TestError>;

fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn file_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> io::Result<String> {
    let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |path: &str| {
        pairs
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, contents)| contents.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
    }
}

#[test]
fn missing_policy_file_env_var_is_an_error() -> TestResult {
    let get_env = env_map(&[]);
    let read_file = file_map(&[]);
    let err = load_config(get_env, read_file).err().ok_or("expected an error")?;
    assert!(err.contains(POLICY_FILE_ENV_VAR));
    Ok(())
}

#[test]
fn listen_addr_defaults_when_unset() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(get_env, read_file)?;
    assert_eq!(config.listen_addr.as_deref(), Some(DEFAULT_LISTEN_ADDR));
    assert_eq!(config.policy_file, "/policy.json");
    assert_eq!(config.policy, NetworkPolicy::AllowAll);
    Ok(())
}

#[test]
fn listen_addr_uses_env_var_when_set() -> TestResult {
    let get_env = env_map(&[
        (LISTEN_ADDR_ENV_VAR, "127.0.0.1:9999"),
        (POLICY_FILE_ENV_VAR, "/policy.json"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""deny_all""#)]);
    let config = load_config(get_env, read_file)?;
    assert_eq!(config.listen_addr.as_deref(), Some("127.0.0.1:9999"));
    Ok(())
}

fn flags(args: &[&str]) -> Result<ListenFlags, String> {
    ListenFlags::parse(args.iter().map(|arg| arg.to_string()))
}

fn base_config() -> Result<Config, String> {
    let get_env = env_map(&[
        (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (MUX_LISTEN_ADDR_ENV_VAR, "127.0.0.1:3128"),
        (MUX_TOKEN_ENV_VAR, "token"),
    ]);
    load_config(get_env, file_map(&[("/policy.json", r#""deny_all""#)]))
}

#[test]
fn listen_flags_parse_and_reject_unknown_arguments() -> TestResult {
    assert_eq!(flags(&[])?, ListenFlags::default());
    let parsed = flags(&["--listen", "127.0.0.1:8080", "--listen-any-interface"])?;
    assert_eq!(parsed.listen.as_deref(), Some("127.0.0.1:8080"));
    assert!(parsed.any_interface);
    assert_eq!(flags(&["--listen-fd", "7"])?.listen_fd, Some(7));
    assert!(flags(&["--listen-fd", "1"]).is_err());
    assert!(flags(&["--listen-fd", "x"]).is_err());
    assert!(flags(&["--listen"]).is_err());
    assert!(flags(&["--nope"]).is_err());
    assert!(flags(&["--listen-fd", "7", "--listen", "127.0.0.1:1"]).is_err());
    Ok(())
}

#[test]
fn listen_refuses_a_non_loopback_address_unless_any_interface_is_given() -> TestResult {
    let none = |_: &str| None;
    for addr in ["0.0.0.0:3128", "192.168.1.5:3128", "[::]:3128", "example.com:80"] {
        let err = flags(&["--listen", addr])?
            .apply(base_config()?, none)
            .err()
            .ok_or("expected an error")?;
        assert_eq!(
            err,
            format!(
                "--listen \"{addr}\" is not a loopback address (127.0.0.0/8, [::1] or localhost); pass --listen-any-interface to bind it anyway"
            )
        );
        let config = flags(&["--listen", addr, "--listen-any-interface"])?.apply(base_config()?, none)?;
        assert_eq!(
            config.mux.ok_or("expected a value")?.listen_addr,
            ListenAddress::Tcp(addr.to_string())
        );
    }
    for addr in ["127.0.0.1:0", "127.9.9.9:80", "[::1]:80", "localhost:80"] {
        flags(&["--listen", addr])?.apply(base_config()?, none)?;
    }
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
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
    let config = load_config(get_env, file_map(&[("/policy.json", r#""deny_all""#)]))?;
    let err = flags(&["--listen", "127.0.0.1:0"])?
        .apply(config.clone(), |_| None)
        .err()
        .ok_or("expected an error")?;
    assert!(err.contains(MUX_TOKEN_ENV_VAR), "{err}");
    let with_token = flags(&["--listen", "127.0.0.1:0"])?
        .apply(config, |key| (key == MUX_TOKEN_ENV_VAR).then(|| "token".to_string()))?;
    assert!(with_token.mux.is_some());
    Ok(())
}

#[test]
fn transparent_listener_can_be_turned_off_when_the_explicit_proxy_is_configured() -> TestResult {
    let get_env = env_map(&[
        (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (MUX_LISTEN_ADDR_ENV_VAR, "unix:/run/p.sock"),
        (MUX_TOKEN_ENV_VAR, "token"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""deny_all""#)]);
    let config = load_config(get_env, read_file)?;
    assert_eq!(config.listen_addr, None);
    assert_eq!(
        config.mux.ok_or("expected a value")?.listen_addr,
        ListenAddress::Unix("/run/p.sock".into())
    );
    Ok(())
}

#[test]
fn turning_the_transparent_listener_off_without_the_explicit_proxy_is_an_error() -> TestResult {
    let get_env = env_map(&[
        (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
        (POLICY_FILE_ENV_VAR, "/policy.json"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""deny_all""#)]);
    assert!(load_config(get_env, read_file).is_err());
    Ok(())
}

#[test]
fn policy_file_read_failure_is_reported() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/does-not-exist.json")]);
    let read_file = file_map(&[]);
    let err = load_config(get_env, read_file).err().ok_or("expected an error")?;
    assert!(err.contains("/does-not-exist.json"));
    Ok(())
}

#[test]
fn policy_file_invalid_json_is_reported() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
    let read_file = file_map(&[("/policy.json", "not json")]);
    let err = load_config(get_env, read_file).err().ok_or("expected an error")?;
    assert!(err.contains("/policy.json"));
    Ok(())
}

#[test]
fn user_defined_policy_parses() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
    let json = r#"
        {
            "user_defined": {
                "rules": [{"domain": "*.example.com", "action": "pass_through"}],
                "allow_ranges": [],
                "deny_ranges": []
            }
        }
    "#;
    let files = [("/policy.json", json)];
    let read_file = file_map(&files);
    let config = load_config(get_env, read_file)?;
    assert!(matches!(config.policy, NetworkPolicy::UserDefined { .. }));
    Ok(())
}

#[test]
fn policy_file_opaque_tunnels_default_to_refuse_and_allow_is_an_explicit_opt_in() -> TestResult {
    let parse = |json: &str| -> Result<NetworkPolicy, String> {
        let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
        let files = [("/policy.json", json)];
        Ok(load_config(get_env, file_map(&files))?.policy)
    };
    let refuses = |policy: &NetworkPolicy| match policy {
        NetworkPolicy::UserDefined { opaque_tunnels, .. } => opaque_tunnels.refuses(),
        _ => false,
    };
    let base = r#"{"user_defined":{"rules":[],"allow_ranges":[],"deny_ranges":[]"#;
    assert!(refuses(&parse(&format!(r#"{base},"opaque_tunnels":"refuse"}}}}"#))?));
    assert!(refuses(&parse(&format!("{base}}}}}"))?));
    assert!(!refuses(&parse(&format!(r#"{base},"opaque_tunnels":"allow"}}}}"#))?));
    Ok(())
}

#[test]
fn oidc_identity_config_is_none_when_all_three_vars_unset() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(get_env, read_file)?;
    assert_eq!(config.oidc_identity, None);
    Ok(())
}

#[test]
fn oidc_identity_config_parses_when_all_three_vars_set() -> TestResult {
    let get_env = env_map(&[
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (OIDC_TENANT_ID_ENV_VAR, "tenant-1"),
        (OIDC_LEASE_ID_ENV_VAR, "lease-1"),
        (OIDC_SUBJECT_ENV_VAR, "sandbox-1"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(get_env, read_file)?;
    let oidc = config.oidc_identity.ok_or("expected a value")?;
    assert_eq!(oidc.tenant_id, TenantId::new("tenant-1")?);
    assert_eq!(oidc.lease_id, LeaseId::new("lease-1"));
    assert_eq!(oidc.subject, SandboxId::new("sandbox-1")?);
    assert_eq!(oidc.signing_key_pem_file, None);
    Ok(())
}

#[test]
fn oidc_identity_config_includes_signing_key_pem_file_when_set() -> TestResult {
    let get_env = env_map(&[
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (OIDC_TENANT_ID_ENV_VAR, "tenant-1"),
        (OIDC_LEASE_ID_ENV_VAR, "lease-1"),
        (OIDC_SUBJECT_ENV_VAR, "sandbox-1"),
        (OIDC_SIGNING_KEY_PEM_FILE_ENV_VAR, "/signing-key.pem"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(get_env, read_file)?;
    let oidc = config.oidc_identity.ok_or("expected a value")?;
    assert_eq!(oidc.signing_key_pem_file, Some("/signing-key.pem".to_string()));
    Ok(())
}

#[test]
fn oidc_identity_config_errors_when_only_some_vars_set() -> TestResult {
    let get_env = env_map(&[
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (OIDC_TENANT_ID_ENV_VAR, "tenant-1"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let err = load_config(get_env, read_file).err().ok_or("expected an error")?;
    assert!(err.contains(OIDC_TENANT_ID_ENV_VAR));
    assert!(err.contains(OIDC_LEASE_ID_ENV_VAR));
    assert!(err.contains(OIDC_SUBJECT_ENV_VAR));
    Ok(())
}

#[test]
fn policy_has_forward_rule_is_false_for_allow_all() -> TestResult {
    assert!(!policy_has_forward_rule(&NetworkPolicy::AllowAll));
    Ok(())
}

#[test]
fn policy_has_forward_rule_is_true_when_a_forward_rule_is_present() -> TestResult {
    let json = r#"
        {
            "user_defined": {
                "rules": [{
                    "domain": "*.example.com",
                    "action": {
                        "forward": {
                            "forward_url": "https://hooks.example.com/receive",
                            "match_rules": [],
                            "ttl_seconds": 60
                        }
                    }
                }],
                "allow_ranges": [],
                "deny_ranges": []
            }
        }
    "#;
    let policy: NetworkPolicy = serde_json::from_str(json)?;
    assert!(policy_has_forward_rule(&policy));
    Ok(())
}

#[test]
fn policy_has_forward_rule_is_false_when_only_pass_through_rules_present() -> TestResult {
    let json = r#"
        {
            "user_defined": {
                "rules": [{"domain": "*.example.com", "action": "pass_through"}],
                "allow_ranges": [],
                "deny_ranges": []
            }
        }
    "#;
    let policy: NetworkPolicy = serde_json::from_str(json)?;
    assert!(!policy_has_forward_rule(&policy));
    Ok(())
}

fn unique_temp_path(name: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("{name}-{nanos}-{count}"))
}

fn test_oidc_identity_context() -> Result<OidcIdentityContext, TestError> {
    Ok(OidcIdentityContext {
        signer: OidcSigner::generate()?,
        issuer: OIDC_ISSUER.to_string(),
        subject: SandboxId::new("test-subject")?,
        tenant_id: TenantId::new("test-tenant")?,
        lease_id: LeaseId::new("test-lease"),
    })
}

fn test_tls_intercept_context() -> Result<Arc<TlsInterceptContext>, TestError> {
    install_default_crypto_provider();
    let ca = SandboxCa::generate(PermittedDomains::from_rules(&[DomainRule {
        domain: "*.example.com".parse()?,
        action: DomainRuleAction::PassThrough,
    }])?)?;
    Ok(Arc::new(TlsInterceptContext::new(
        ca,
        system_upstream_tls_config(),
        Arc::new(UnconfiguredCredentialSource),
        test_oidc_identity_context()?,
    )))
}

/// Accepts one connection on `proxy_listener` and drives it through the
/// exact same "read-clone the shared policy, then hand off to
/// `handle_connection`" sequence used by `handle_accepted_connection`
/// (destination recovery is skipped here since `SO_ORIGINAL_DST` is
/// Linux-only and the test destination is already known), returning the
/// resulting outcome.
async fn drive_one_connection_through_shared_policy(
    proxy_listener: &TcpListener,
    shared: &SharedPolicy,
    tls_intercept: &Arc<TlsInterceptContext>,
    upstream_addr: SocketAddr,
) -> Result<trogon_sandbox_egress_proxy::proxy::ProxyOutcome, TestError> {
    let (server_stream, _peer_addr) = proxy_listener.accept().await?;

    let mut policy = shared.read().await.clone();
    if let NetworkPolicy::UserDefined { allow_ranges, .. } = &mut policy {
        allow_ranges.push(trogon_sandbox_contract::AllowRange::new("127.0.0.0/8")?);
    }
    Ok(handle_connection(
        server_stream,
        &policy,
        upstream_addr.ip(),
        upstream_addr.port(),
        &VettingResolver::system(),
        tls_intercept,
    )
    .await)
}

fn policy_json_with_rule(domain: &str, action_json: &str) -> String {
    format!(
        r#"{{"user_defined": {{"rules": [{{"domain": "{domain}", "action": {action_json}}}], "allow_ranges": [], "deny_ranges": []}}}}"#
    )
}

const FORWARD_ACTION_JSON: &str =
    r#"{"forward": {"forward_url": "https://hooks.example.com/receive", "match_rules": [], "ttl_seconds": 60}}"#;

fn permitted_example_com() -> Result<PermittedDomains, TestError> {
    Ok(PermittedDomains::from_rules(&[DomainRule {
        domain: "*.example.com".parse()?,
        action: DomainRuleAction::PassThrough,
    }])?)
}

#[test]
fn intercept_domains_outside_ca_reports_only_uncovered_intercept_rules() -> TestResult {
    let permitted = permitted_example_com()?;
    let parse = |json: String| serde_json::from_str::<NetworkPolicy>(&json);

    let inside = parse(policy_json_with_rule("api.example.com", FORWARD_ACTION_JSON))?;
    assert!(intercept_domains_outside_ca(&inside, &permitted).is_empty());

    let outside = parse(policy_json_with_rule("*.other.org", FORWARD_ACTION_JSON))?;
    assert_eq!(
        intercept_domains_outside_ca(&outside, &permitted),
        vec!["*.other.org".to_string()]
    );

    let pass_through = parse(policy_json_with_rule("*.other.org", r#""pass_through""#))?;
    assert!(intercept_domains_outside_ca(&pass_through, &permitted).is_empty());

    assert!(intercept_domains_outside_ca(&NetworkPolicy::AllowAll, &permitted).is_empty());
    Ok(())
}

#[tokio::test]
async fn hot_reload_rejects_out_of_constraint_intercept_domain_and_keeps_applying_valid_ones() -> TestResult {
    let policy_file = unique_temp_path("sandbox-egress-policy-reload-constraint-test");
    std::fs::write(&policy_file, r#""deny_all""#)?;

    let handle = PolicyHandle::new(NetworkPolicy::DenyAll, permitted_example_com()?);
    let shared = handle.shared();
    let poll_interval = Duration::from_millis(100);
    let _reload_task = spawn_policy_reload_task(policy_file.to_string_lossy().into_owned(), handle, poll_interval);

    let write_policy = |contents: &str| -> io::Result<()> {
        let swap = unique_temp_path("sandbox-egress-policy-reload-constraint-swap");
        std::fs::write(&swap, contents)?;
        std::fs::rename(&swap, &policy_file)
    };

    write_policy(&policy_json_with_rule("*.other.org", FORWARD_ACTION_JSON))?;
    tokio::time::sleep(poll_interval * 4).await;
    assert_eq!(*shared.read().await, NetworkPolicy::DenyAll);

    write_policy(&policy_json_with_rule("api.example.com", FORWARD_ACTION_JSON))?;
    tokio::time::sleep(poll_interval * 4).await;
    let applied = shared.read().await.clone();
    assert!(policy_has_forward_rule(&applied));

    let _ = std::fs::remove_file(&policy_file);
    Ok(())
}

#[tokio::test]
async fn hot_reload_applies_updated_policy_without_restart() -> TestResult {
    let policy_file = unique_temp_path("sandbox-egress-policy-hot-reload-test");
    std::fs::write(&policy_file, r#""deny_all""#)?;

    let handle = PolicyHandle::new(NetworkPolicy::DenyAll, PermittedDomains::from_rules(&[])?);
    let shared = handle.shared();
    let poll_interval = Duration::from_millis(100);
    let _reload_task = spawn_policy_reload_task(policy_file.to_string_lossy().into_owned(), handle, poll_interval);
    let tls_intercept = test_tls_intercept_context()?;

    let upstream_listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = upstream_listener.local_addr()?;
    let payload = b"hello upstream, this is plain tcp, not tls at all";
    let upstream_task = tokio::spawn(async move {
        let (mut sock, _) = upstream_listener.accept().await?;
        let mut buf = vec![0u8; payload.len()];
        sock.read_exact(&mut buf).await?;
        sock.write_all(&buf).await?;
        Ok::<_, io::Error>(buf)
    });

    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;

    let mut first_client = TcpStream::connect(proxy_addr).await?;
    let _ = first_client.write_all(payload).await;

    let denied_outcome =
        drive_one_connection_through_shared_policy(&proxy_listener, &shared, &tls_intercept, upstream_addr).await?;
    assert!(matches!(
        denied_outcome,
        trogon_sandbox_egress_proxy::proxy::ProxyOutcome::Denied(_)
    ));
    drop(first_client);

    let updated_policy_file = unique_temp_path("sandbox-egress-policy-hot-reload-test-swap");
    std::fs::write(
        &updated_policy_file,
        r#"{"user_defined": {"rules": [], "allow_ranges": ["127.0.0.0/8"], "deny_ranges": [], "opaque_tunnels": "allow"}}"#,
    )?;
    std::fs::rename(&updated_policy_file, &policy_file)?;

    tokio::time::sleep(poll_interval * 3).await;

    let mut second_client = TcpStream::connect(proxy_addr).await?;
    let write_task = tokio::spawn(async move {
        second_client.write_all(payload).await?;
        let mut echoed = vec![0u8; payload.len()];
        second_client.read_exact(&mut echoed).await?;
        Ok::<_, io::Error>(echoed)
    });

    let allowed_outcome =
        drive_one_connection_through_shared_policy(&proxy_listener, &shared, &tls_intercept, upstream_addr).await?;

    match allowed_outcome {
        trogon_sandbox_egress_proxy::proxy::ProxyOutcome::Completed { .. } => {}
        other => return Err(format!("expected Completed outcome after reload, got {other:?}").into()),
    }

    let echoed = write_task.await??;
    assert_eq!(&echoed, payload);
    let received_by_upstream = upstream_task.await??;
    assert_eq!(&received_by_upstream, payload);

    let _ = std::fs::remove_file(&policy_file);
    Ok(())
}

#[test]
fn upstream_exemption_defaults_to_marking() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(get_env, read_file)?;
    assert_eq!(config.upstream_exemption, UpstreamExemption::Mark);
    Ok(())
}

#[test]
fn upstream_exemption_by_uid_parses_the_exempt_uid() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json"), (EXEMPT_UID_ENV_VAR, "65531")]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(get_env, read_file)?;
    assert_eq!(
        config.upstream_exemption,
        UpstreamExemption::Uid(trogon_sandbox_contract::ProxyUid::new(65531))
    );
    Ok(())
}

#[test]
fn a_malformed_exempt_uid_fails_startup_config() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json"), (EXEMPT_UID_ENV_VAR, "nobody")]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let err = load_config(get_env, read_file).err().ok_or("expected an error")?;
    assert!(err.contains(EXEMPT_UID_ENV_VAR));
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the failure is an anyhow error whose message is its only distinguishing data"
    )
)]
async fn run_fails_closed_when_the_process_uid_is_not_the_exempt_uid() -> TestResult {
    let get_env = env_map(&[
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (LISTEN_ADDR_ENV_VAR, "127.0.0.1:0"),
        (EXEMPT_UID_ENV_VAR, "65531"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let mut config = load_config(get_env, read_file)?;
    let actual = dial::effective_uid()?;
    config.upstream_exemption =
        UpstreamExemption::Uid(trogon_sandbox_contract::ProxyUid::new(actual.get().wrapping_add(1)));
    let err = run(config).await.err().ok_or("expected an error")?;
    assert!(err.to_string().contains(EXEMPT_UID_ENV_VAR), "{err}");
    Ok(())
}

#[test]
fn violation_events_config_requires_both_vars() -> TestResult {
    let get_env = env_map(&[
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (VIOLATION_NATS_URL_ENV_VAR, "nats://localhost:4222"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let err = load_config(get_env, read_file).err().ok_or("expected an error")?;
    assert!(err.contains(VIOLATION_LEASE_ID_ENV_VAR));
    Ok(())
}

#[test]
fn violation_events_config_parses_when_both_vars_set() -> TestResult {
    let get_env = env_map(&[
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (VIOLATION_NATS_URL_ENV_VAR, "nats://localhost:4222"),
        (VIOLATION_LEASE_ID_ENV_VAR, "lease-1"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(get_env, read_file)?;
    assert_eq!(
        config.violation_events,
        Some(ViolationEventsConfig {
            nats_url: "nats://localhost:4222".to_string(),
            lease_id: LeaseId::new("lease-1"),
        })
    );
    Ok(())
}

fn decider_env(extra: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let mut pairs = vec![
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (VIOLATION_NATS_URL_ENV_VAR, "nats://localhost:4222"),
        (VIOLATION_LEASE_ID_ENV_VAR, "lease-1"),
    ];
    pairs.extend_from_slice(extra);
    env_map(&pairs)
}

#[test]
fn decider_is_off_unless_a_timeout_is_set() -> TestResult {
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(decider_env(&[]), read_file)?;
    assert_eq!(config.decider, None);
    Ok(())
}

#[test]
fn decider_config_reads_timeout_and_defaults_the_cap() -> TestResult {
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(decider_env(&[(DECIDER_TIMEOUT_MS_ENV_VAR, "1500")]), read_file)?;
    assert_eq!(
        config.decider,
        Some(DeciderConfig {
            nats_url: "nats://localhost:4222".to_string(),
            lease_id: LeaseId::new("lease-1"),
            limits: DeciderLimits {
                timeout: Duration::from_millis(1500),
                max_in_flight: DeciderLimits::default().max_in_flight,
            },
        })
    );
    Ok(())
}

#[test]
fn decider_config_reads_the_in_flight_cap() -> TestResult {
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let config = load_config(
        decider_env(&[
            (DECIDER_TIMEOUT_MS_ENV_VAR, "1000"),
            (DECIDER_MAX_IN_FLIGHT_ENV_VAR, "4"),
        ]),
        read_file,
    )?;
    assert_eq!(config.decider.ok_or("expected a value")?.limits.max_in_flight, 4);
    Ok(())
}

#[test]
fn a_supplied_ca_needs_both_files() -> TestResult {
    let policy = [("/policy.json", r#""allow_all""#)];
    let both = load_config(
        decider_env(&[
            (CA_CERT_FILE_ENV_VAR, "/ca/cert.pem"),
            (CA_KEY_FILE_ENV_VAR, "/ca/key.pem"),
        ]),
        file_map(&policy),
    )?;
    assert_eq!(
        both.supplied_ca,
        Some(("/ca/cert.pem".to_string(), "/ca/key.pem".to_string()))
    );
    let one = load_config(
        decider_env(&[(CA_CERT_FILE_ENV_VAR, "/ca/cert.pem")]),
        file_map(&policy),
    );
    assert!(one.err().ok_or("expected an error")?.contains("must be set together"));
    Ok(())
}

#[test]
fn decider_config_rejects_unusable_values() -> TestResult {
    let cases: [&[(&str, &str)]; 3] = [
        &[(DECIDER_TIMEOUT_MS_ENV_VAR, "0")],
        &[(DECIDER_TIMEOUT_MS_ENV_VAR, "soon")],
        &[
            (DECIDER_TIMEOUT_MS_ENV_VAR, "1000"),
            (DECIDER_MAX_IN_FLIGHT_ENV_VAR, "0"),
        ],
    ];
    for extra in cases {
        let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
        assert!(load_config(decider_env(extra), read_file).is_err());
    }
    Ok(())
}

#[test]
fn decider_config_needs_the_violation_nats_settings() -> TestResult {
    let get_env = env_map(&[
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (DECIDER_TIMEOUT_MS_ENV_VAR, "1000"),
    ]);
    let read_file = file_map(&[("/policy.json", r#""allow_all""#)]);
    let err = load_config(get_env, read_file).err().ok_or("expected an error")?;
    assert!(err.contains(VIOLATION_NATS_URL_ENV_VAR));
    Ok(())
}

#[derive(Default)]
struct CollectingSink {
    events: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
}

#[async_trait::async_trait]
impl trogon_sandbox_egress_proxy::violation::ViolationSink for CollectingSink {
    async fn publish(
        &self,
        subject: String,
        payload: Vec<u8>,
    ) -> Result<(), trogon_sandbox_egress_proxy::violation::ViolationPublishError> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((subject, payload));
        Ok(())
    }
}

#[tokio::test]
async fn each_denied_connection_publishes_one_violation_event() -> TestResult {
    let shared: SharedPolicy = Arc::new(RwLock::new(NetworkPolicy::DenyAll));
    let tls_intercept = test_tls_intercept_context()?;
    let sink = Arc::new(CollectingSink::default());
    let reporter = ViolationReporter::new(LeaseId::new("lease-1"), sink.clone(), ViolationRateLimit::default());
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    let dest: SocketAddr = "203.0.113.7:443".parse()?;

    for _ in 0..2 {
        let mut client = TcpStream::connect(proxy_addr).await?;
        let _ = client.write_all(b"hello, not tls and not postgres").await;
        let outcome =
            drive_one_connection_through_shared_policy(&proxy_listener, &shared, &tls_intercept, dest).await?;
        reporter.report_outcome(&outcome, dest).await;
    }

    let events = sink.events.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "sandbox.egress.violation.lease-1");
    let violation: trogon_sandbox_contract::EgressViolation = serde_json::from_slice(&events[0].1)?;
    assert_eq!(violation.dest_ip, Some(dest.ip()));
    assert_eq!(violation.port, 443);
    Ok(())
}

const TRANSFORM_ACTION_JSON: &str = r#"{"transform": {"credential": {"provider": "registry", "pull_secret_id": "pull-secret", "scope": {"kind": "registry_pull", "repository_glob": "*"}, "ttl_seconds": 60}, "injection": {"header": "authorization"}}}"#;

fn sink_logs() -> tracing::subscriber::DefaultGuard {
    tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(std::io::sink)
            .finish(),
    )
}

fn env_with(base: &[(&str, &str)], extra: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let mut pairs = extra.to_vec();
    pairs.extend_from_slice(base);
    env_map(&pairs)
}

fn config_with(extra: &[(&str, &str)], policy: &str) -> Result<Config, String> {
    let base = [
        (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (MUX_LISTEN_ADDR_ENV_VAR, "127.0.0.1:0"),
        (MUX_TOKEN_ENV_VAR, "token"),
    ];
    load_config(env_with(&base, extra), file_map(&[("/policy.json", policy)]))
}

fn load_error(extra: &[(&str, &str)]) -> Result<String, TestError> {
    config_with(extra, r#""deny_all""#)
        .err()
        .ok_or_else(|| "expected an error".into())
}

#[test]
fn empty_listen_flags_leave_the_config_untouched() -> TestResult {
    let config = base_config()?;
    let applied = ListenFlags::default().apply(config.clone(), |_| None)?;
    assert_eq!(applied, config);
    Ok(())
}

#[test]
fn listen_flags_without_an_address_keep_the_default_explicit_address() -> TestResult {
    let get_env = env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]);
    let config = load_config(get_env, file_map(&[("/policy.json", r#""deny_all""#)]))?;
    let applied = flags(&["--listen-any-interface"])?
        .apply(config, |key| (key == MUX_TOKEN_ENV_VAR).then(|| "token".to_string()))?;
    assert_eq!(
        applied.mux.ok_or("expected a value")?.listen_addr,
        ListenAddress::Tcp(DEFAULT_MUX_LISTEN_ADDR.to_string())
    );
    Ok(())
}

#[test]
fn a_listen_address_without_a_port_is_checked_as_a_host() -> TestResult {
    assert!(is_loopback_listen("localhost"));
    assert!(is_loopback_listen("127.0.0.1"));
    assert!(is_loopback_listen("[::1]:80"));
    assert!(!is_loopback_listen("example.com"));
    Ok(())
}

#[test]
fn only_transform_rules_name_credential_providers_once() -> TestResult {
    let two = format!(
        r#"{{"user_defined": {{"rules": [{{"domain": "a.example.com", "action": {TRANSFORM_ACTION_JSON}}}, {{"domain": "b.example.com", "action": {TRANSFORM_ACTION_JSON}}}, {{"domain": "c.example.com", "action": "pass_through"}}], "allow_ranges": [], "deny_ranges": []}}}}"#
    );
    let policy: NetworkPolicy = serde_json::from_str(&two)?;
    assert_eq!(
        referenced_credential_providers(&policy),
        vec![CredentialProvider::Registry]
    );
    assert!(referenced_credential_providers(&NetworkPolicy::AllowAll).is_empty());
    Ok(())
}

#[test]
fn credential_broker_settings_are_read_together() -> TestResult {
    let config = config_with(
        &[
            (CREDENTIAL_BROKER_NATS_URL_ENV_VAR, "nats://broker:4222"),
            (CREDENTIAL_BROKER_TENANT_ID_ENV_VAR, "tenant-1"),
            (CREDENTIAL_BROKER_LEASE_ID_ENV_VAR, "lease-1"),
            (CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR, "7"),
        ],
        r#""deny_all""#,
    )?;
    let broker = config.credential_broker.ok_or("expected a broker")?;
    assert_eq!(broker.tenant, TenantId::new("tenant-1")?);
    assert_eq!(
        broker.fence,
        Some(PolicyFence {
            lease_id: LeaseId::new("lease-1"),
            policy_revision: 7
        })
    );
    Ok(())
}

#[test]
fn incomplete_credential_broker_settings_are_refused() -> TestResult {
    let revision_not_a_number = load_error(&[
        (CREDENTIAL_BROKER_LEASE_ID_ENV_VAR, "lease-1"),
        (CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR, "next"),
    ])?;
    assert!(revision_not_a_number.contains(CREDENTIAL_BROKER_POLICY_REVISION_ENV_VAR));
    let lone_lease = load_error(&[(CREDENTIAL_BROKER_LEASE_ID_ENV_VAR, "lease-1")])?;
    assert!(lone_lease.contains(CREDENTIAL_BROKER_LEASE_ID_ENV_VAR));
    let lone_url = load_error(&[(CREDENTIAL_BROKER_NATS_URL_ENV_VAR, "nats://broker:4222")])?;
    assert!(lone_url.contains(CREDENTIAL_BROKER_TENANT_ID_ENV_VAR));
    let lone_tenant = load_error(&[(CREDENTIAL_BROKER_TENANT_ID_ENV_VAR, "tenant-1")])?;
    assert!(lone_tenant.contains(CREDENTIAL_BROKER_NATS_URL_ENV_VAR));
    let unusable_tenant = load_error(&[
        (CREDENTIAL_BROKER_NATS_URL_ENV_VAR, "nats://broker:4222"),
        (CREDENTIAL_BROKER_TENANT_ID_ENV_VAR, ""),
    ])?;
    assert!(unusable_tenant.contains(CREDENTIAL_BROKER_TENANT_ID_ENV_VAR));
    Ok(())
}

#[test]
fn lone_mux_settings_and_bad_ca_pairs_are_refused() -> TestResult {
    let only_token = load_config(
        env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json"), (MUX_TOKEN_ENV_VAR, "token")]),
        file_map(&[("/policy.json", r#""deny_all""#)]),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(only_token.contains(MUX_LISTEN_ADDR_ENV_VAR));
    let unusable_token = load_error(&[(MUX_TOKEN_ENV_VAR, "")])?;
    assert!(unusable_token.contains(MUX_TOKEN_ENV_VAR));
    let secrets_fd = load_error(&[(SECRETS_FD_ENV_VAR, "stdin")])?;
    assert!(secrets_fd.contains(SECRETS_FD_ENV_VAR));
    let secrets_fd = config_with(&[(SECRETS_FD_ENV_VAR, "9")], r#""deny_all""#)?;
    assert_eq!(secrets_fd.secrets_fd, Some(9));
    Ok(())
}

#[cfg(unix)]
fn secret_pipe(contents: &[u8]) -> Result<i32, TestError> {
    let (reader, mut writer) = std::io::pipe()?;
    writer.write_all(contents)?;
    drop(writer);
    Ok(std::os::fd::OwnedFd::from(reader).into_raw_fd())
}

#[cfg(unix)]
#[test]
fn the_secret_handoff_is_read_from_the_inherited_descriptor() -> TestResult {
    let wire = RegistryHandoff::from_registry(&SentinelRegistry::new())?;
    let registry = read_secret_registry(secret_pipe(wire.expose_wire().as_bytes())?)?;
    assert_eq!(registry.len(), 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_secret_handoff_that_does_not_decode_or_read_is_refused() -> TestResult {
    let malformed = read_secret_registry(secret_pipe(b"not a handoff")?);
    assert!(matches!(malformed, Err(err) if err.kind() == io::ErrorKind::Other));
    let unreadable = read_secret_registry(secret_pipe(&[0xff, 0xfe])?);
    assert!(matches!(unreadable, Err(err) if err.kind() == io::ErrorKind::Other));
    Ok(())
}

#[test]
fn the_process_environment_and_policy_files_are_read_through_the_standard_sources() -> TestResult {
    assert_eq!(process_env("TROGON_EGRESS_PROXY_TEST_UNSET_VARIABLE"), None);
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("policy.json");
    std::fs::write(&file, "\"deny_all\"")?;
    assert_eq!(read_policy_file(&file.to_string_lossy())?, "\"deny_all\"");
    assert!(read_policy_file(&dir.path().join("missing.json").to_string_lossy()).is_err());
    Ok(())
}

#[tokio::test]
async fn serving_with_an_invalid_configuration_exits_with_failure() -> TestResult {
    let _logs = sink_logs();
    let code = serve(
        ["--bogus".to_string()],
        env_map(&[(POLICY_FILE_ENV_VAR, "/policy.json")]),
        file_map(&[("/policy.json", r#""deny_all""#)]),
    )
    .await;
    assert_eq!(code, 1);
    Ok(())
}

#[tokio::test]
async fn serving_exits_with_failure_when_the_proxy_cannot_start() -> TestResult {
    let _logs = sink_logs();
    let forward = policy_json_with_rule("api.example.com", FORWARD_ACTION_JSON);
    let code = serve(
        Vec::<String>::new(),
        env_map(&[
            (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
            (POLICY_FILE_ENV_VAR, "/policy.json"),
            (MUX_LISTEN_ADDR_ENV_VAR, "127.0.0.1:0"),
            (MUX_TOKEN_ENV_VAR, "token"),
        ]),
        file_map(&[("/policy.json", forward.as_str())]),
    )
    .await;
    assert_eq!(code, 1);
    Ok(())
}

#[cfg(unix)]
async fn crl_is_served<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(mut stream: S) -> io::Result<bool> {
    stream
        .write_all(b"GET /ca.crl HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    Ok(reply.starts_with(b"HTTP/1.1 200"))
}

#[cfg(unix)]
async fn serving_on_socket(config: Config, socket: &std::path::Path) -> TestResult {
    let probe = async {
        for _ in 0..1200 {
            if let Ok(stream) = tokio::net::UnixStream::connect(socket).await
                && crl_is_served(stream).await.unwrap_or(false)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err::<(), TestError>("the proxy never served".into())
    };
    tokio::select! {
        stopped = run(config) => Err(format!("the proxy stopped early: {stopped:?}").into()),
        served = probe => served,
    }
}

#[cfg(unix)]
fn socket_env(dir: &std::path::Path) -> (std::path::PathBuf, String) {
    let socket = dir.join("proxy.sock");
    let address = format!("unix:{}", socket.display());
    (socket, address)
}

#[cfg(unix)]
async fn serves(extra: &[(&str, &str)], policy: &str) -> TestResult {
    let _logs = sink_logs();
    let dir = tempfile::tempdir()?;
    let (socket, address) = socket_env(dir.path());
    let mut pairs = vec![(MUX_LISTEN_ADDR_ENV_VAR, address.as_str())];
    pairs.extend_from_slice(extra);
    let base = [
        (LISTEN_ADDR_ENV_VAR, TRANSPARENT_LISTENER_OFF),
        (POLICY_FILE_ENV_VAR, "/policy.json"),
        (MUX_TOKEN_ENV_VAR, "token"),
    ];
    let config = load_config(env_with(&base, &pairs), file_map(&[("/policy.json", policy)]))?;
    serving_on_socket(config, &socket).await
}

#[cfg(unix)]
#[tokio::test]
async fn a_default_proxy_serves_on_its_socket_and_writes_the_ca() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ca_out = dir.path().join("ca.pem");
    serves(&[(CA_OUT_FILE_ENV_VAR, &ca_out.to_string_lossy())], r#""allow_all""#).await?;
    assert!(std::fs::read_to_string(&ca_out)?.contains("BEGIN CERTIFICATE"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_supplied_ca_signs_for_the_proxy() -> TestResult {
    let dir = tempfile::tempdir()?;
    let generated = trogon_sandbox_egress_proxy::tls_ca::generate_ca(None, None)?;
    let cert = dir.path().join("ca.pem");
    let key = dir.path().join("ca.key");
    std::fs::write(&cert, generated.cert_pem)?;
    std::fs::write(&key, generated.key_pem)?;
    serves(
        &[
            (CA_CERT_FILE_ENV_VAR, &cert.to_string_lossy()),
            (CA_KEY_FILE_ENV_VAR, &key.to_string_lossy()),
        ],
        r#""allow_all""#,
    )
    .await
}

#[tokio::test]
async fn a_supplied_ca_that_cannot_be_loaded_stops_the_proxy() -> TestResult {
    let _logs = sink_logs();
    let config = config_with(
        &[
            (CA_CERT_FILE_ENV_VAR, "/nonexistent/ca.pem"),
            (CA_KEY_FILE_ENV_VAR, "/nonexistent/ca.key"),
        ],
        r#""allow_all""#,
    )?;
    let result = run(config).await;
    assert!(matches!(result, Err(err) if err.kind() == io::ErrorKind::Other));
    Ok(())
}

#[tokio::test]
async fn a_forward_rule_without_an_oidc_identity_stops_the_proxy() -> TestResult {
    let _logs = sink_logs();
    let config = config_with(&[], &policy_json_with_rule("api.example.com", FORWARD_ACTION_JSON))?;
    let result = run(config).await;
    assert!(matches!(result, Err(err) if err.kind() == io::ErrorKind::Other));
    Ok(())
}

const OIDC_IDENTITY: [(&str, &str); 3] = [
    (OIDC_TENANT_ID_ENV_VAR, "tenant-1"),
    (OIDC_LEASE_ID_ENV_VAR, "lease-1"),
    (OIDC_SUBJECT_ENV_VAR, "sandbox-1"),
];

#[cfg(unix)]
#[tokio::test]
async fn an_oidc_identity_with_a_signing_key_file_lets_forward_rules_serve() -> TestResult {
    let dir = tempfile::tempdir()?;
    let key = dir.path().join("oidc.pem");
    std::fs::write(&key, rcgen::KeyPair::generate()?.serialize_pem())?;
    let mut extra = OIDC_IDENTITY.to_vec();
    let key_path = key.to_string_lossy().into_owned();
    extra.push((OIDC_SIGNING_KEY_PEM_FILE_ENV_VAR, &key_path));
    serves(&extra, &policy_json_with_rule("api.example.com", FORWARD_ACTION_JSON)).await
}

#[cfg(unix)]
#[tokio::test]
async fn an_oidc_identity_without_a_key_file_generates_its_own_key() -> TestResult {
    serves(
        &OIDC_IDENTITY,
        &policy_json_with_rule("api.example.com", FORWARD_ACTION_JSON),
    )
    .await
}

#[tokio::test]
async fn an_oidc_signing_key_file_that_is_missing_or_unusable_stops_the_proxy() -> TestResult {
    let _logs = sink_logs();
    let dir = tempfile::tempdir()?;
    let unusable = dir.path().join("oidc.pem");
    std::fs::write(&unusable, "not a key")?;
    let missing = dir.path().join("missing.pem");
    for key in [&unusable, &missing] {
        let mut extra = OIDC_IDENTITY.to_vec();
        let key_path = key.to_string_lossy().into_owned();
        extra.push((OIDC_SIGNING_KEY_PEM_FILE_ENV_VAR, &key_path));
        let config = config_with(&extra, &policy_json_with_rule("api.example.com", FORWARD_ACTION_JSON))?;
        assert!(run(config).await.is_err());
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_proxy_with_a_transparent_listener_and_a_uid_exemption_serves() -> TestResult {
    let _logs = sink_logs();
    let dir = tempfile::tempdir()?;
    let (socket, address) = socket_env(dir.path());
    let mut config = config_with(&[], r#""allow_all""#)?;
    if let Some(mux) = config.mux.as_mut() {
        mux.listen_addr = ListenAddress::parse(&address);
    }
    config.listen_addr = Some("127.0.0.1:0".to_string());
    config.upstream_exemption = UpstreamExemption::Uid(dial::effective_uid()?);
    serving_on_socket(config, &socket).await
}

#[tokio::test]
async fn a_transparent_listener_that_cannot_mark_its_upstream_sockets_stops_the_proxy() -> TestResult {
    let _logs = sink_logs();
    if trogon_sandbox_egress_proxy::dial::UpstreamMarking::Required
        .verify()
        .is_ok()
    {
        return Ok(());
    }
    let mut config = config_with(&[], r#""allow_all""#)?;
    config.listen_addr = Some("127.0.0.1:0".to_string());
    let result = run(config).await;
    assert!(matches!(result, Err(err) if err.kind() == io::ErrorKind::Other));
    Ok(())
}

#[tokio::test]
async fn a_transparent_address_that_cannot_be_bound_stops_the_proxy() -> TestResult {
    let _logs = sink_logs();
    let mut config = config_with(&[], r#""allow_all""#)?;
    config.listen_addr = Some("not an address".to_string());
    assert!(run(config).await.is_err());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn an_inherited_listening_descriptor_serves_the_explicit_proxy() -> TestResult {
    let _logs = sink_logs();
    let inherited = std::net::TcpListener::bind("127.0.0.1:0")?;
    inherited.set_nonblocking(true)?;
    let addr = inherited.local_addr()?;
    let mut config = config_with(&[], r#""allow_all""#)?;
    if let Some(mux) = config.mux.as_mut() {
        mux.inherited_fd = Some(inherited.into_raw_fd());
    }
    let probe = async {
        for _ in 0..1200 {
            if let Ok(stream) = TcpStream::connect(addr).await
                && crl_is_served(stream).await.unwrap_or(false)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err::<(), TestError>("the proxy never served".into())
    };
    tokio::select! {
        stopped = run(config) => Err(format!("the proxy stopped early: {stopped:?}").into()),
        served = probe => served,
    }
}

#[cfg(unix)]
mod secret_handoff_and_broker_tests;

#[tokio::test]
async fn nats_endpoints_that_cannot_be_reached_stop_the_proxy() -> TestResult {
    let _logs = sink_logs();
    let policy = policy_json_with_rule("api.example.com", TRANSFORM_ACTION_JSON);
    let broker = config_with(
        &[
            (CREDENTIAL_BROKER_NATS_URL_ENV_VAR, "not a url"),
            (CREDENTIAL_BROKER_TENANT_ID_ENV_VAR, "tenant-1"),
        ],
        &policy,
    )?;
    assert!(run(broker).await.is_err());
    let violations = config_with(
        &[
            (VIOLATION_NATS_URL_ENV_VAR, "not a url"),
            (VIOLATION_LEASE_ID_ENV_VAR, "lease-1"),
        ],
        r#""allow_all""#,
    )?;
    assert!(run(violations).await.is_err());
    let decider = config_with(
        &[
            (VIOLATION_NATS_URL_ENV_VAR, "not a url"),
            (VIOLATION_LEASE_ID_ENV_VAR, "lease-1"),
            (DECIDER_TIMEOUT_MS_ENV_VAR, "1000"),
        ],
        r#""allow_all""#,
    )?;
    assert!(run(decider).await.is_err());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_policy_reload_reports_files_it_cannot_stat_read_or_parse_and_keeps_serving() -> TestResult {
    let captured = Captured::default();
    let _logs = tracing::subscriber::set_default(captured.subscriber());
    let dir = tempfile::tempdir()?;
    let handle = PolicyHandle::new(NetworkPolicy::DenyAll, PermittedDomains::from_rules(&[])?);
    let shared = handle.shared();
    let missing = dir.path().join("missing.json");
    let unreadable = dir.path().join("directory");
    std::fs::create_dir(&unreadable)?;
    let unparsable = dir.path().join("unparsable.json");
    std::fs::write(&unparsable, "{")?;
    let interval = Duration::from_secs(1);
    let tasks = [&missing, &unreadable, &unparsable]
        .map(|path| spawn_policy_reload_task(path.to_string_lossy().into_owned(), handle.clone(), interval));

    tokio::time::sleep(interval * 3).await;
    for task in &tasks {
        task.abort();
    }

    let text = captured.text();
    assert!(text.contains("failed to stat policy file"), "{text}");
    assert!(text.contains("failed to read policy file"), "{text}");
    assert!(text.contains("failed to parse policy file"), "{text}");
    assert_eq!(*shared.read().await, NetworkPolicy::DenyAll);
    Ok(())
}

#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())).into_owned()
    }

    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let writer = self.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish()
    }
}
