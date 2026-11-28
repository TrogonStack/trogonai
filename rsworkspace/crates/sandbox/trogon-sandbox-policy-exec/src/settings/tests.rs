use super::*;
use trogon_sandbox_contract::AmbiguousTargets;
use trogon_sandbox_contract::Port;
use trogon_std::env::ReadEnv;

fn document(network: &str, filesystem: &str) -> String {
    format!(r#"{{"network": {network}, "filesystem": {filesystem}}}"#)
}

fn with_domains(allowed: &str, denied: &str) -> String {
    document(
        &format!(r#"{{"allowedDomains": {allowed}, "deniedDomains": {denied}}}"#),
        r#"{"denyRead": [], "allowWrite": [], "denyWrite": []}"#,
    )
}

fn with_fs(deny_read: &str, allow_write: &str, deny_write: &str) -> String {
    document(
        r#"{"allowedDomains": [], "deniedDomains": []}"#,
        &format!(r#"{{"denyRead": {deny_read}, "allowWrite": {allow_write}, "denyWrite": {deny_write}}}"#),
    )
}

fn with_extra(key: &str, value: &str) -> String {
    format!(
        r#"{{"network": {{"allowedDomains": [], "deniedDomains": []}},
            "filesystem": {{"denyRead": [], "allowWrite": [], "denyWrite": []}},
            "{key}": {value}}}"#
    )
}

fn with_network_extra(key: &str, value: &str) -> String {
    document(
        &format!(r#"{{"allowedDomains": [], "deniedDomains": [], "{key}": {value}}}"#),
        r#"{"denyRead": [], "allowWrite": [], "denyWrite": []}"#,
    )
}

fn with_filesystem_extra(key: &str, value: &str) -> String {
    document(
        r#"{"allowedDomains": [], "deniedDomains": []}"#,
        &format!(r#"{{"denyRead": [], "allowWrite": [], "denyWrite": [], "{key}": {value}}}"#),
    )
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn accepted(json: &str) -> Result<SandboxSettings, Box<dyn std::error::Error>> {
    SandboxSettings::from_json(json).map_err(|err| format!("{json} was refused: {err}").into())
}

fn refused(json: &str) -> Result<SettingsError, Box<dyn std::error::Error>> {
    SandboxSettings::from_json(json)
        .err()
        .ok_or_else(|| format!("{json} was accepted").into())
}

#[test]
fn a_minimal_config_is_valid() -> TestResult {
    accepted(&with_domains("[]", "[]"))?;
    Ok(())
}

#[test]
fn a_minimal_config_denies_the_network_and_adds_no_fs_rules() -> TestResult {
    let policy = accepted(&with_domains("[]", "[]"))?.policy()?;
    assert!(policy.fs.is_none());
    match policy.network {
        NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            opaque_tunnels,
            ..
        } => {
            assert!(rules.is_empty() && allow_ranges.is_empty() && deny_ranges.is_empty());
            assert!(denied_domains.is_empty());
            assert_eq!(opaque_tunnels, OpaqueTunnelPolicy::Allow);
        }
        other => return Err(format!("expected a user defined policy, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn domains_become_rules_and_denied_domains() -> TestResult {
    let settings = accepted(&with_domains(r#"["example.com", "*.github.com"]"#, r#"["evil.com"]"#))?;
    match settings.policy()?.network {
        NetworkPolicy::UserDefined {
            rules, denied_domains, ..
        } => {
            assert_eq!(rules.len(), 2);
            assert_eq!(denied_domains.len(), 1);
        }
        other => return Err(format!("expected a user defined policy, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn valid_wildcard_patterns_are_accepted() -> TestResult {
    for domain in ["*.example.com", "*.github.io", "*.co.uk"] {
        accepted(&with_domains(&format!(r#"["{domain}"]"#), "[]"))?;
    }
    Ok(())
}

#[test]
fn invalid_domain_patterns_are_refused() -> TestResult {
    for domain in [
        "",
        "example",
        "*example.com",
        "*.com",
        "*.",
        "http://example.com",
        "example.com/path",
        "not a domain",
    ] {
        let err = refused(&with_domains(&format!(r#"["{domain}"]"#), "[]"))?;
        assert_eq!(err.path(), "network.allowedDomains[0]", "{domain:?}");
    }
    Ok(())
}

#[test]
fn a_refused_domain_entry_carries_the_documented_message() -> TestResult {
    let err = refused(&with_domains(r#"["*.com"]"#, "[]"))?;
    assert_eq!(err.path(), "network.allowedDomains[0]");
    assert_eq!(err.message(), trogon_sandbox_protocol::ALLOWED_DOMAIN_ENTRY_MESSAGE);
    let err = refused(&with_domains("[]", r#"["not a domain"]"#))?;
    assert_eq!(err.path(), "network.deniedDomains[0]");
    assert_eq!(err.message(), trogon_sandbox_protocol::DENIED_DOMAIN_ENTRY_MESSAGE);
    Ok(())
}

#[test]
fn valid_port_suffixes_are_accepted() -> TestResult {
    for domain in [
        "example.com:443",
        "[::1]",
        "[::1]:443",
        "[2001:db8::1]:8443",
        "*.example.com:8080",
        "localhost:3000",
        "example.com:65535",
        "example.com:1",
    ] {
        accepted(&with_domains(&format!(r#"["{domain}"]"#), "[]"))?;
    }
    Ok(())
}

#[test]
fn invalid_port_suffixes_are_refused() -> TestResult {
    for domain in [
        "example.com:0",
        "example.com:65536",
        "example.com:0443",
        "example.com:abc",
        "example.com:80:443",
        "example.com:",
        "*:22",
        "::1",
        "fd00:ec2::254",
        "2001:db8::1:443",
        "[::1",
        "[::1]:0",
        "[not-an-ip]",
    ] {
        refused(&with_domains(&format!(r#"["{domain}"]"#), "[]"))?;
    }
    Ok(())
}

#[test]
fn a_bare_star_is_allowed_only_in_denied_domains() -> TestResult {
    accepted(&with_domains("[]", r#"["*:22"]"#))?;
    accepted(&with_domains("[]", r#"["*"]"#))?;
    Ok(())
}

#[test]
fn empty_filesystem_paths_are_refused() -> TestResult {
    for (list, json) in [
        ("denyRead", with_fs(r#"[""]"#, "[]", "[]")),
        ("allowWrite", with_fs("[]", r#"[""]"#, "[]")),
        ("denyWrite", with_fs("[]", "[]", r#"[""]"#)),
    ] {
        let err = refused(&json)?;
        assert_eq!(err.path(), format!("filesystem.{list}[0]"));
    }
    Ok(())
}

#[test]
fn relative_and_home_paths_are_refused() -> TestResult {
    for path in ["relative/file", "~/.ssh", "./here"] {
        refused(&with_fs(&format!(r#"["{path}"]"#), "[]", "[]"))?;
    }
    Ok(())
}

#[test]
fn missing_required_fields_are_refused_with_their_path() -> TestResult {
    assert_eq!(refused("{}")?.path(), "network");
    assert_eq!(refused(r#"{"network": {}}"#)?.path(), "network.allowedDomains");
    assert_eq!(
        refused(r#"{"network": {"allowedDomains": [], "deniedDomains": []}}"#)?.path(),
        "filesystem"
    );
    assert_eq!(
        refused(r#"{"network": {"allowedDomains": [], "deniedDomains": []}, "filesystem": {"denyRead": []}}"#)?.path(),
        "filesystem.allowWrite"
    );
    Ok(())
}

#[test]
fn wrong_types_are_refused() -> TestResult {
    refused(r#"{"network": "not-an-object", "filesystem": {}}"#)?;
    refused(&with_domains(r#""example.com""#, "[]"))?;
    refused(&with_extra("ignoreViolations", r#"{"*": "not-a-list"}"#))?;
    Ok(())
}

#[test]
fn unknown_keys_are_refused_at_every_level() -> TestResult {
    let root = refused(&with_extra("surprise", "true"))?;
    assert_eq!(root.to_string(), "(root): Unrecognized key(s) in object: 'surprise'");
    let network = refused(&with_network_extra("surprise", "true"))?;
    assert_eq!(
        network.to_string(),
        "network: Unrecognized key(s) in object: 'surprise'"
    );
    let filesystem = refused(&with_filesystem_extra("surprise", "true"))?;
    assert_eq!(
        filesystem.to_string(),
        "filesystem: Unrecognized key(s) in object: 'surprise'"
    );
    Ok(())
}

#[test]
fn every_failing_option_is_reported_in_the_schema_order() -> TestResult {
    assert_eq!(refused("{}")?.to_string(), "network: Required; filesystem: Required");
    let json = r#"{
        "enableWeakerNestedSandbox": "yes",
        "filesystem": {"denyRead": 3, "allowWrite": []},
        "network": {"allowedDomains": "a.com", "deniedDomains": [1]}
    }"#;
    assert_eq!(
        refused(json)?.to_string(),
        "network.allowedDomains: Expected array, received string; \
         network.deniedDomains.0: Expected string, received number; \
         filesystem.denyRead: Expected array, received number; \
         filesystem.denyWrite: Required; \
         enableWeakerNestedSandbox: Expected boolean, received string"
    );
    Ok(())
}

#[test]
fn a_document_that_is_no_object_is_reported_at_the_root() -> TestResult {
    assert_eq!(refused("5")?.to_string(), "(root): Expected object, received number");
    assert_eq!(refused("null")?.to_string(), "(root): Expected object, received null");
    Ok(())
}

#[test]
fn missing_options_inside_a_list_entry_are_each_reported() -> TestResult {
    let json = r#"{
        "network": {"allowedDomains": [], "deniedDomains": []},
        "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
        "credentials": {"files": [{}]}
    }"#;
    assert_eq!(
        refused(json)?.to_string(),
        "credentials.files.0.path: Required; credentials.files.0.mode: Required"
    );
    Ok(())
}

#[test]
fn several_unknown_keys_of_one_object_are_listed_together() -> TestResult {
    let json = r#"{
        "network": {"allowedDomains": [], "deniedDomains": [], "a": 1, "b": 2},
        "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []}
    }"#;
    assert_eq!(
        refused(json)?.to_string(),
        "network: Unrecognized key(s) in object: 'a', 'b'"
    );
    Ok(())
}

#[test]
fn the_lenient_reader_reports_every_failing_option_too() -> TestResult {
    let json = r#"{"network": {"extra": 1}, "filesystem": {"denyRead": 3}}"#;
    let err = SandboxSettings::from_json_lenient(json, None).err().ok_or(json)?;
    assert_eq!(
        err.to_string(),
        "network.allowedDomains: Required; network.deniedDomains: Required; \
         filesystem.denyRead: Expected array, received number; filesystem.allowWrite: Required; \
         filesystem.denyWrite: Required"
    );
    Ok(())
}

#[test]
fn glob_and_literal_entries_are_told_apart() -> TestResult {
    let policy = accepted(&with_fs(
        r#"["/data/*.pem", "/etc/shadow", {"path": "/odd/*name", "literal": true}]"#,
        r#"["/tmp"]"#,
        "[]",
    ))?
    .policy()?
    .fs
    .ok_or("no filesystem policy")?;
    assert!(matches!(policy.deny_read()[0], FsPathEntry::Glob { .. }));
    assert!(matches!(policy.deny_read()[1], FsPathEntry::Literal { .. }));
    assert!(matches!(policy.deny_read()[2], FsPathEntry::Literal { .. }));
    assert_eq!(policy.allow_write().len(), 1);
    Ok(())
}

#[test]
fn literal_entries_need_literal_true() -> TestResult {
    refused(&with_fs(r#"[{"path": "/x", "literal": false}]"#, "[]", "[]"))?;
    refused(&with_fs(r#"[{"path": "/x"}]"#, "[]", "[]"))?;
    refused(&with_fs(r#"[{"literal": true}]"#, "[]", "[]"))?;
    refused(&with_fs(r#"[{"path": "/x", "literal": true, "extra": 1}]"#, "[]", "[]"))?;
    Ok(())
}

#[test]
fn allow_read_becomes_the_allow_within_deny_list() -> TestResult {
    let settings = accepted(&with_filesystem_extra("allowRead", r#"["/etc/hosts"]"#))?;
    let fs = settings.policy()?.fs.ok_or("no filesystem policy")?;
    assert_eq!(fs.allow_read_within_deny().len(), 1);
    Ok(())
}

#[test]
fn deny_globs_that_match_nothing_are_refused() -> TestResult {
    for glob in ["/data/*/", "/data/**/", "/data/?/"] {
        let err = refused(&with_fs(&format!(r#"["{glob}"]"#), "[]", "[]"))?;
        assert_eq!(err.path(), "filesystem.denyRead[0]");
        assert!(
            err.message().contains(&format!(
                "Deny glob \"{glob}\" ends in a separator, so as a pattern it can match no path. Write \"{}\"",
                glob.trim_end_matches('/')
            )),
            "{err}"
        );
    }
    let err = refused(&with_fs("[]", "[]", r#"["/ok", "/data/[ab]/"]"#))?;
    assert_eq!(err.path(), "filesystem.denyWrite[1]");
    Ok(())
}

#[test]
fn a_disabled_filesystem_accepts_the_deny_globs_that_match_nothing() -> TestResult {
    let json = document(
        r#"{"allowedDomains": [], "deniedDomains": []}"#,
        r#"{"denyRead": ["/data/*/"], "allowWrite": [], "denyWrite": ["/data/*/"], "disabled": true}"#,
    );
    let fs = accepted(&json)?.policy()?.fs;
    assert!(fs.is_none_or(|fs| fs.deny_read().is_empty() && fs.deny_write().is_empty()));
    Ok(())
}

#[test]
fn literal_entries_with_a_trailing_slash_are_fine() -> TestResult {
    accepted(&with_fs(r#"[{"path": "/data/dir/", "literal": true}]"#, "[]", "[]"))?.policy()?;
    Ok(())
}

#[test]
fn bwrap_path_must_be_absolute_and_non_empty() -> TestResult {
    assert_eq!(
        accepted(&with_extra("bwrapPath", r#""/usr/bin/bwrap""#))?.bwrap_path(),
        Some(Path::new("/usr/bin/bwrap"))
    );
    assert_eq!(refused(&with_extra("bwrapPath", r#""""#))?.path(), "bwrapPath");
    assert_eq!(refused(&with_extra("bwrapPath", r#""bwrap""#))?.path(), "bwrapPath");
    Ok(())
}

#[test]
fn boolean_options_must_be_booleans() -> TestResult {
    refused(&with_network_extra("refuseOpaqueTunnels", r#""yes""#))?;
    refused(&with_extra("allowPty", r#""yes""#))?;
    Ok(())
}

#[test]
fn refuse_opaque_tunnels_maps_to_the_policy() -> TestResult {
    let settings = accepted(&with_network_extra("refuseOpaqueTunnels", "true"))?;
    match settings.policy()?.network {
        NetworkPolicy::UserDefined { opaque_tunnels, .. } => {
            assert_eq!(opaque_tunnels, OpaqueTunnelPolicy::Refuse)
        }
        other => return Err(format!("expected a user defined policy, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn denied_resolved_addresses_accept_addresses_and_ranges() -> TestResult {
    let settings = accepted(&with_network_extra(
        "deniedResolvedAddresses",
        r#"["10.0.0.0/8", "192.168.1.1", "fd00::/8", "::1"]"#,
    ))?;
    let options = settings.network_options_with_env(&NoEnv)?;
    let ranges: Vec<_> = options
        .denied_resolved_addresses
        .iter()
        .map(|range| range.as_str())
        .collect();
    assert_eq!(ranges, ["10.0.0.0/8", "192.168.1.1/32", "fd00::/8", "::1/128"]);
    refused(&with_network_extra("deniedResolvedAddresses", r#"["not-an-ip"]"#))?;
    Ok(())
}

#[test]
fn ignore_violations_rules_are_carried_over() -> TestResult {
    let settings = accepted(&with_extra(
        "ignoreViolations",
        r#"{"*": ["/tmp/noise"], "git push": ["ssh"]}"#,
    ))?;
    let ignore = settings.ignore_violations();
    assert!(ignore.ignores("deny file-read /tmp/noise", None));
    assert!(ignore.ignores("deny network ssh", Some("git push")));
    assert!(!ignore.ignores("deny network ssh", Some("git pull")));
    Ok(())
}

#[test]
fn proxy_ports_in_range_select_an_external_proxy() -> TestResult {
    for port in [1u16, 8080, 65535] {
        let http = network_options_of(&format!(r#""httpProxyPort": {port}"#))?
            .external_proxy
            .ok_or("a value was absent")?;
        assert_eq!(http.http_port().map(Port::get), Some(port));
        assert_eq!(http.socks_port(), None);
        let socks = network_options_of(&format!(r#""socksProxyPort": {port}"#))?
            .external_proxy
            .ok_or("a value was absent")?;
        assert_eq!(socks.socks_port().map(Port::get), Some(port));
        assert_eq!(socks.http_port(), None);
    }
    let both = network_options_of(r#""httpProxyPort": 3128, "socksProxyPort": 3128"#)?;
    assert_eq!(
        both.external_proxy
            .ok_or("a value was absent")?
            .http_port()
            .map(Port::get),
        Some(3128)
    );
    assert!(network_options_of(r#""tlsTerminate": {}"#)?.external_proxy.is_none());
    Ok(())
}

#[test]
fn proxy_ports_out_of_range_or_mistyped_are_refused() -> TestResult {
    for key in ["httpProxyPort", "socksProxyPort"] {
        for port in ["0", "65536", "-1", r#""80""#, "1.5"] {
            let err = refused(&with_network_extra(key, port))?;
            assert_eq!(err.path(), format!("network.{key}"), "{key} {port}");
        }
    }
    Ok(())
}

#[test]
fn differing_http_and_socks_ports_reach_the_options_apart() -> TestResult {
    let options = network_options_of(r#""httpProxyPort": 3128, "socksProxyPort": 1080"#)?;
    let external = options.external_proxy.ok_or("a value was absent")?;
    assert_eq!(external.http_port().map(Port::get), Some(3128));
    assert_eq!(external.socks_port().map(Port::get), Some(1080));
    Ok(())
}

#[test]
fn one_external_port_leaves_the_other_protocol_to_the_runtime() -> TestResult {
    let options = network_options_of(r#""socksProxyPort": 1080"#)?;
    let external = options.external_proxy.ok_or("no external proxy")?;
    assert_eq!(external.http_port(), None);
    assert_eq!(external.socks_port().map(Port::get), Some(1080));
    Ok(())
}

#[test]
fn external_proxies_for_both_protocols_cannot_terminate_tls_or_mask_credentials() -> TestResult {
    for tls in [
        r#"{"caCertPath": "/ca.pem", "caKeyPath": "/ca.key"}"#,
        r#"{"excludeDomains": ["a.com"]}"#,
        r#"{"extraCaCertPaths": ["/x.pem"]}"#,
    ] {
        let json = with_network_extra_pairs(&format!(
            r#""httpProxyPort": 3128, "socksProxyPort": 1080, "tlsTerminate": {tls}"#
        ));
        let err = refused(&json)?;
        assert!(err.path().starts_with("network."), "{tls}: {}", err.path());
    }
    Ok(())
}

#[test]
fn ca_files_reach_the_network_options_together() -> TestResult {
    let options = network_options_of(r#""tlsTerminate": {"caCertPath": "/ca.pem", "caKeyPath": "/ca.key"}"#)?;
    let files = options.ca_files.ok_or("a value was absent")?;
    assert_eq!(files.cert_path(), Path::new("/ca.pem"));
    assert_eq!(files.key_path(), Path::new("/ca.key"));
    assert!(network_options_of(r#""tlsTerminate": {}"#)?.ca_files.is_none());
    Ok(())
}

#[test]
fn tls_termination_and_a_mitm_proxy_are_mutually_exclusive() -> TestResult {
    let both =
        with_network_extra_pairs(r#""tlsTerminate": {}, "mitmProxy": {"socketPath": "/s", "domains": ["a.com"]}"#);
    let err = refused(&both)?;
    assert_eq!(
        err.to_string(),
        "network.tlsTerminate and network.mitmProxy are mutually exclusive"
    );
    accepted(&with_network_extra("tlsTerminate", "{}"))?;
    accepted(&with_network_extra(
        "mitmProxy",
        r#"{"socketPath": "/s", "domains": ["a.com"]}"#,
    ))?;
    Ok(())
}

#[test]
fn a_single_ca_path_is_refused() -> TestResult {
    for tls in [r#"{"caCertPath": "/ca.pem"}"#, r#"{"caKeyPath": "/ca.key"}"#] {
        let err = refused(&with_network_extra("tlsTerminate", tls))?;
        assert_eq!(err.path(), "network.tlsTerminate", "{tls}");
        assert!(err.message().contains("together"), "{err}");
    }
    Ok(())
}

#[test]
fn exclude_domains_reach_the_tls_exclusions() -> TestResult {
    let options =
        network_options_of(r#""tlsTerminate": {"excludeDomains": ["pinned.example.com", "*.cdn.example.org"]}"#)?;
    let excluded = options.tls_exclusions.as_slice();
    assert_eq!(excluded.len(), 2);
    assert!(excluded.contains(&DomainPattern::new("pinned.example.com")?));
    assert!(excluded.contains(&DomainPattern::new("*.cdn.example.org")?));
    assert!(!excluded.contains(&DomainPattern::new("other.example.com")?));
    Ok(())
}

#[test]
fn an_allowed_ip_literal_is_carved_out_of_the_private_range_floor() -> TestResult {
    let policy = accepted(&with_domains(r#"["10.1.2.3", "example.com"]"#, "[]"))?.policy()?;
    assert!(format!("{policy:?}").contains("10.1.2.3"), "{policy:?}");
    Ok(())
}

#[test]
fn an_allowed_ip_literal_with_a_port_is_carved_out_for_that_port_only() -> TestResult {
    let policy = accepted(&with_domains(r#"["10.1.2.3:8080", "[fc00::1]:9"]"#, "[]"))?.policy()?;
    let NetworkPolicy::UserDefined {
        allow_ranges, rules, ..
    } = &policy.network
    else {
        return Err(format!("{policy:?}").into());
    };
    assert!(allow_ranges.is_empty(), "{allow_ranges:?}");
    let rendered = format!("{rules:?}");
    assert!(rendered.contains("10.1.2.3"), "{rendered}");
    assert!(rendered.contains("fc00::1"), "{rendered}");
    Ok(())
}

#[test]
fn options_this_runtime_cannot_enforce_are_refused_when_they_ask_for_more() -> TestResult {
    let network = [("parentProxy", r#"{"url": "http://p:1"}"#)];
    for (key, value) in network {
        let err = refused(&with_network_extra(key, value))?;
        assert!(
            err.path().starts_with(&format!("network.{key}")),
            "{key}: {}",
            err.path()
        );
    }
    let top = [
        ("mandatoryDenySearchDepth", "0"),
        ("mandatoryDenySearchDepth", "11"),
        ("socatPath", r#""socat""#),
        ("javaAgentJarPath", r#""x.jar""#),
    ];
    for (key, value) in top {
        assert_eq!(refused(&with_extra(key, value))?.path(), key);
    }
    assert_eq!(refused(&with_extra("ripgrep", "{}"))?.path(), "ripgrep.command");
    assert_eq!(
        refused(&with_extra("seccomp", r#"{"unknown": 1}"#))?.to_string(),
        "seccomp: Unrecognized key(s) in object: 'unknown'"
    );
    Ok(())
}

#[test]
fn unix_sockets_apple_events_nested_namespaces_and_filesystem_switches_are_accepted() -> TestResult {
    accepted(&with_network_extra("allowUnixSockets", r#"["/var/run/docker.sock"]"#))?;
    accepted(&with_network_extra("allowAllUnixSockets", "true"))?;
    accepted(&with_extra("allowNestedUserNamespaces", "true"))?;
    accepted(&with_extra("allowAppleEvents", "true"))?;
    accepted(&with_extra("windows", r#"{"sandboxUser": "x"}"#))?;
    accepted(&with_filesystem_extra("disabled", "true"))?;
    accepted(&with_filesystem_extra("allowGitConfig", "true"))?;
    Ok(())
}

#[test]
fn the_tool_paths_are_absolute_and_reach_the_config_and_the_runtime() -> TestResult {
    let settings = accepted(&with_extra("socatPath", r#""/usr/bin/socat""#))?;
    assert_eq!(settings.socat_path(), Some(Path::new("/usr/bin/socat")));
    let config = settings.to_config("/work")?;
    assert_eq!(config.socat_path(), Some(Path::new("/usr/bin/socat")));
    let settings = accepted(&with_extra("javaAgentJarPath", r#""/opt/agent.jar""#))?;
    assert_eq!(settings.java_agent_jar_path(), Some(Path::new("/opt/agent.jar")));
    Ok(())
}

#[test]
fn the_java_agent_jar_is_readable_inside_a_read_deny_when_it_exists() -> TestResult {
    let dir = tempfile::tempdir()?;
    let jar = dir.path().join("agent.jar");
    std::fs::write(&jar, b"jar")?;
    let settings = accepted(&format!(
        r#"{{"network":{{"allowedDomains":[],"deniedDomains":[]}},"filesystem":{{"denyRead":["/secret"],"allowWrite":[],"denyWrite":[]}},"javaAgentJarPath":{}}}"#,
        serde_json::to_string(&jar)?
    ))?;
    let fs = settings
        .to_config("/work")?
        .policy()
        .fs
        .clone()
        .ok_or("a value was absent")?;
    assert!(fs.allow_read_within_deny().iter().any(|entry| matches!(
        entry,
        FsPathEntry::Literal { path, .. } if Some(path.as_str()) == jar.to_str()
    )));
    Ok(())
}

#[test]
fn the_standard_config_resolves_private_names_and_the_default_does_not() -> TestResult {
    let settings = accepted(&with_extra("socatPath", r#""/usr/bin/socat""#))?;
    let plain = settings.to_config("/work")?;
    assert!(!plain.configured_network_options().private_ranges_by_name.is_enabled());
    let preset = settings.to_standard_config("/work")?;
    assert!(preset.configured_network_options().private_ranges_by_name.is_enabled());
    assert_eq!(preset.socat_path(), Some(Path::new("/usr/bin/socat")));
    Ok(())
}

#[test]
fn the_standard_config_resolves_loopback_names_and_matches_names_by_name() -> TestResult {
    let settings = accepted(&with_extra("socatPath", r#""/usr/bin/socat""#))?;
    let plain = settings.to_config("/work")?;
    assert!(!plain.configured_network_options().loopback_names.is_enabled());
    assert!(!plain.configured_network_options().names_matched_by_name.is_enabled());
    let preset = settings.to_standard_config("/work")?;
    assert!(preset.configured_network_options().loopback_names.is_enabled());
    assert!(preset.configured_network_options().names_matched_by_name.is_enabled());
    Ok(())
}

#[test]
fn the_standard_config_reports_the_host_match_requirement_only_when_the_settings_ask_for_it() -> TestResult {
    let host_match =
        |json: String| -> Result<Option<HostMatch>, Box<dyn std::error::Error>> { Ok(accepted(&json)?.host_match()) };
    assert_eq!(host_match(with_extra("socatPath", r#""/usr/bin/socat""#))?, None);
    assert_eq!(
        host_match(with_network_extra("requireHostMatch", "true"))?,
        Some(HostMatch::Required)
    );
    assert_eq!(
        host_match(with_network_extra("requireHostMatch", "false"))?,
        Some(HostMatch::NotRequired)
    );
    Ok(())
}

#[test]
fn the_standard_config_anchors_the_macos_protections_at_the_working_directory() -> TestResult {
    let settings = accepted(&with_extra("socatPath", r#""/usr/bin/socat""#))?;
    let anchor = |config: PolicyExecConfig| config.configured_platform_options().macos.protected_names_anchor;
    let expected = if cfg!(target_os = "macos") {
        ProtectedNamesAnchor::WorkingDirectory
    } else {
        ProtectedNamesAnchor::WriteRoots
    };
    assert_eq!(anchor(settings.to_standard_config("/work")?), expected);
    assert_eq!(anchor(settings.to_config("/work")?), ProtectedNamesAnchor::WriteRoots);
    Ok(())
}

#[test]
fn the_standard_config_scans_three_levels_on_linux_unless_the_settings_say_otherwise() -> TestResult {
    let depth = |json: String, preset: bool| -> Result<_, Box<dyn std::error::Error>> {
        let settings = accepted(&json)?;
        let config = if preset {
            settings.to_standard_config("/work")
        } else {
            settings.to_config("/work")
        }?;
        Ok(config
            .configured_platform_options()
            .filesystem
            .mandatory_deny_search_depth)
    };
    let unset = || with_extra("socatPath", r#""/usr/bin/socat""#);
    let three = Some(MandatoryDenySearchDepth::new(3)?);
    let two = Some(MandatoryDenySearchDepth::new(2)?);
    if cfg!(target_os = "linux") {
        assert_eq!(depth(unset(), true)?, three);
        assert_eq!(depth(with_extra("mandatoryDenySearchDepth", "2"), true)?, two);
    } else {
        assert_eq!(depth(unset(), true)?, None);
    }
    assert_eq!(depth(unset(), false)?, None);
    Ok(())
}

#[test]
fn an_empty_unix_socket_entry_loads_and_allows_nothing() -> TestResult {
    let settings = accepted(&with_network_extra("allowUnixSockets", r#"[""]"#))?;
    let options = settings.platform_options_for(true, false);
    assert!(options.macos.allow_unix_sockets.is_empty());
    Ok(())
}

#[test]
fn the_new_switches_reach_the_platform_options() -> TestResult {
    let json = r#"{"network": {"allowedDomains": [], "deniedDomains": [],
            "allowUnixSockets": ["/var/run/docker.sock"], "allowAllUnixSockets": false},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": [],
            "allowGitConfig": true, "disabled": true},
            "allowAppleEvents": true, "allowNestedUserNamespaces": true}"#;
    let settings = accepted(json)?;
    let macos = settings.platform_options_for(true, false);
    assert_eq!(macos.macos.allow_unix_sockets.len(), 1);
    assert!(macos.macos.allow_apple_events.is_allowed());
    assert!(!macos.macos.allow_all_unix_sockets.is_allowed());
    assert!(macos.filesystem.allow_git_config.is_writable() && macos.filesystem.disabled.is_disabled());
    let linux = settings.platform_options_for(false, true);
    assert!(linux.linux.allow_nested_user_namespaces.is_allowed());
    assert!(linux.macos.allow_unix_sockets.is_empty());
    assert!(!linux.macos.allow_apple_events.is_allowed());
    Ok(())
}

#[test]
fn allow_all_unix_sockets_replaces_the_path_list() -> TestResult {
    let settings = accepted(&with_network_extra("allowAllUnixSockets", "true"))?;
    assert!(
        settings
            .platform_options_for(true, false)
            .macos
            .allow_all_unix_sockets
            .is_allowed()
    );
    assert!(
        settings
            .platform_options_for(false, true)
            .linux
            .allow_all_unix_sockets
            .is_allowed()
    );
    Ok(())
}

#[test]
fn lenient_parsing_drops_keys_it_can_do_without() -> TestResult {
    let json = r#"{"network": {"allowedDomains": [], "deniedDomains": [], "extra": 1,
            "mitmProxy": {"socketPath": "/s", "domains": ["a.com"], "more": 1}},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": [], "extra": 2},
            "top": true, "ripgrep": {"command": "rg", "extra": 3}}"#;
    let settings = SandboxSettings::from_json_lenient(json, None)?;
    let dropped: Vec<_> = settings.warnings().iter().map(String::as_str).collect();
    for key in ["top", "network.extra", "filesystem.extra", "ripgrep.extra"] {
        assert!(dropped.iter().any(|line| line.ends_with(key)), "{key}: {dropped:?}");
    }
    Ok(())
}

#[test]
fn lenient_parsing_strips_foreign_keys_inside_credential_entries() -> TestResult {
    let json = r#"{"network": {"allowedDomains": [], "deniedDomains": []},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
            "credentials": {
                "allowPlaintextInject": true,
                "files": [{"path": "/a", "mode": "deny"}, {"path": "/b", "mode": "deny", "foreign": 1}],
                "envVars": [{"name": "TOKEN", "mode": "mask", "foreign": true}]}}"#;
    let settings = SandboxSettings::from_json_lenient(json, None)?;
    let dropped: Vec<_> = settings.warnings().iter().map(String::as_str).collect();
    for key in ["credentials.files[1].foreign", "credentials.envVars[0].foreign"] {
        assert!(dropped.iter().any(|line| line.ends_with(key)), "{key}: {dropped:?}");
    }
    SandboxSettings::from_json(json)
        .err()
        .ok_or("strict parsing still refuses them")?;
    Ok(())
}

#[test]
fn lenient_parsing_keeps_strictness_inside_the_rest_of_credentials() -> TestResult {
    let json = r#"{"network": {"allowedDomains": [], "deniedDomains": []},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
            "credentials": {"awsPairs": [{"accessKeyIdVar": "A", "secretAccessKeyVar": "B", "foreign": 1}]}}"#;
    SandboxSettings::from_json_lenient(json, None)
        .err()
        .ok_or("aws pairs stay strict")?;
    Ok(())
}

#[test]
fn a_handshake_timeout_is_carried_into_the_network_options() -> TestResult {
    let options = network_options_of(r#""tlsTerminate": {"handshakeTimeoutMs": 2500}"#)?;
    assert_eq!(options.handshake_timeout, Some(std::time::Duration::from_millis(2500)));
    assert_eq!(network_options_of(r#""tlsTerminate": {}"#)?.handshake_timeout, None);
    Ok(())
}

#[test]
fn a_handshake_timeout_outside_the_range_is_refused() -> TestResult {
    for millis in ["99", "600001", "0", "-1", "1.5", r#""1000""#] {
        let err = refused(&with_network_extra(
            "tlsTerminate",
            &format!(r#"{{"handshakeTimeoutMs": {millis}}}"#),
        ))?;
        assert!(
            err.path().starts_with("network.tlsTerminate"),
            "{millis}: {}",
            err.path()
        );
    }
    Ok(())
}

#[test]
fn the_ambiguous_request_target_modes_are_carried_into_the_network_options() -> TestResult {
    for (text, mode) in [
        ("refuse", AmbiguousTargets::Refuse),
        ("asSpelled", AmbiguousTargets::AsSpelled),
        ("normalize", AmbiguousTargets::Normalize),
    ] {
        let options = network_options_of(&format!(r#""ambiguousRequestTarget": "{text}""#))?;
        assert_eq!(options.ambiguous_targets, mode, "{text}");
    }
    assert_eq!(
        network_options_of(r#""requireHostMatch": true"#)?.ambiguous_targets,
        AmbiguousTargets::Refuse
    );
    Ok(())
}

#[test]
fn an_unknown_ambiguous_request_target_mode_is_refused() -> TestResult {
    let err = refused(&with_network_extra("ambiguousRequestTarget", r#""lenient""#))?;
    assert!(err.path().starts_with("network.ambiguousRequestTarget"));
    Ok(())
}

#[test]
fn lenient_parsing_keeps_strictness_where_a_value_decides_the_policy() -> TestResult {
    let json = r#"{"network": {"allowedDomains": [], "deniedDomains": []},
            "filesystem": {"denyRead": [{"path": "/a", "literal": true, "extra": 1}],
            "allowWrite": [], "denyWrite": []}}"#;
    SandboxSettings::from_json_lenient(json, None)
        .err()
        .ok_or("path entry stays strict")?;
    Ok(())
}

#[test]
fn strict_parsing_remains_the_default() -> TestResult {
    refused(&with_extra("windows", r#"{"surprise": 1}"#).replace("windows", "surprise"))?;
    Ok(())
}

#[test]
fn options_that_ask_for_nothing_extra_are_accepted() -> TestResult {
    for (key, value) in [
        ("allowUnixSockets", "[]"),
        ("allowAllUnixSockets", "false"),
        ("allowLocalBinding", "false"),
    ] {
        accepted(&with_network_extra(key, value))?;
    }
    for key in ["enableWeakerNestedSandbox", "allowPty", "allowAppleEvents"] {
        accepted(&with_extra(key, "false"))?;
    }
    for key in ["disabled", "allowGitConfig"] {
        accepted(&with_filesystem_extra(key, "false"))?;
    }
    Ok(())
}

#[test]
fn trailing_input_after_the_document_is_refused() -> TestResult {
    refused(&format!("{} {{}}", with_domains("[]", "[]")))?;
    Ok(())
}

#[test]
fn into_config_carries_policy_bwrap_and_ignore_rules() -> TestResult {
    let dir = tempfile::tempdir()?;
    let json = r#"{"network": {"allowedDomains": ["example.com"], "deniedDomains": []},
            "filesystem": {"denyRead": ["/etc/shadow"], "allowWrite": [], "denyWrite": []},
            "bwrapPath": "/usr/bin/bwrap", "ignoreViolations": {"*": ["x"]}}"#;
    let config = accepted(json)?.into_config(dir.path())?;
    assert_eq!(config.bwrap_path(), Some(Path::new("/usr/bin/bwrap")));
    assert!(config.policy().fs.is_some());
    assert!(!config.ignored_violations().is_empty());
    Ok(())
}

fn read_text(text: &str) -> Result<SettingsFile, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("settings.json");
    std::fs::write(&path, text)?;
    Ok(SettingsFile::read(&path))
}

#[test]
fn a_path_that_does_not_exist_is_missing() -> TestResult {
    assert!(matches!(
        SettingsFile::read(Path::new("/nonexistent/path/config.json")),
        SettingsFile::Missing
    ));
    Ok(())
}

#[test]
fn a_directory_is_unreadable_not_missing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let file = SettingsFile::read(dir.path());
    assert!(matches!(file, SettingsFile::Unreadable(_)));
    let problem = file.problem(dir.path()).ok_or("no problem reported")?;
    assert!(problem.contains("could not be read"));
    assert!(problem.contains("EISDIR"), "{problem}");
    Ok(())
}

#[test]
fn empty_and_whitespace_only_files_are_empty() -> TestResult {
    assert!(matches!(read_text("")?, SettingsFile::Empty));
    assert!(matches!(read_text("   \n\t  ")?, SettingsFile::Empty));
    Ok(())
}

#[test]
fn invalid_json_is_named_as_such() -> TestResult {
    let file = read_text("{ invalid json }")?;
    let problem = file.problem(Path::new("/x.json")).ok_or("no problem reported")?;
    assert!(problem.contains("is not valid JSON"), "{problem}");
    Ok(())
}

#[test]
fn a_schema_mismatch_names_the_failing_key() -> TestResult {
    let file = read_text(r#"{"network": {}}"#)?;
    let problem = file.problem(Path::new("/x.json")).ok_or("no problem reported")?;
    assert!(problem.contains("does not hold a valid config"), "{problem}");
    assert!(problem.contains("network.allowedDomains"), "{problem}");
    Ok(())
}

#[test]
fn a_valid_file_loads() -> TestResult {
    let file = read_text(&with_domains(r#"["example.com"]"#, "[]"))?;
    assert!(matches!(file, SettingsFile::Loaded(_)));
    assert!(file.problem(Path::new("/x.json")).is_none());
    Ok(())
}
struct NoEnv;

impl ReadEnv for NoEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }
}

struct OneVar(&'static str, &'static str);

impl ReadEnv for OneVar {
    fn var(&self, key: &str) -> Result<String, std::env::VarError> {
        if key == self.0 {
            Ok(self.1.to_string())
        } else {
            Err(std::env::VarError::NotPresent)
        }
    }
}

fn network_options_of(extra: &str) -> Result<NetworkOptions, Box<dyn std::error::Error>> {
    network_options_with(extra, &NoEnv)
}

fn network_options_with(extra: &str, env: &impl ReadEnv) -> Result<NetworkOptions, Box<dyn std::error::Error>> {
    let json = with_network_extra_pairs(extra);
    Ok(accepted(&json)?.network_options_with_env(env)?)
}

fn with_network_extra_pairs(pairs: &str) -> String {
    document(
        &format!(r#"{{"allowedDomains": [], "deniedDomains": ["blocked.test"], {pairs}}}"#),
        r#"{"denyRead": [], "allowWrite": [], "denyWrite": []}"#,
    )
}

#[test]
fn allow_plaintext_header_set_is_off_unless_asked_for() -> TestResult {
    assert!(
        !network_options_of(r#""requireHostMatch": false"#)?
            .plaintext_header_set
            .is_enabled()
    );
    assert!(
        !network_options_of(r#""allowPlaintextHeaderSet": false"#)?
            .plaintext_header_set
            .is_enabled()
    );
    assert!(
        network_options_of(r#""allowPlaintextHeaderSet": true"#)?
            .plaintext_header_set
            .is_enabled()
    );
    Ok(())
}

#[test]
fn a_config_without_proxy_options_has_the_default_network_options() -> TestResult {
    let settings = accepted(&with_domains("[]", "[]"))?;
    assert_eq!(settings.network_options()?, NetworkOptions::default());
    assert_eq!(settings.platform_options(), PlatformOptions::default());
    Ok(())
}

#[test]
fn denied_domain_reasons_become_the_refusal_words() -> TestResult {
    let options = network_options_of(r#""deniedDomainReasons": {"blocked.test": "use the mirror"}"#)?;
    let entry = DeniedDomain::new("blocked.test")?;
    assert_eq!(
        options.denied_domain_reasons.get(&entry).map(|reason| reason.as_str()),
        Some("use the mirror")
    );
    Ok(())
}

#[test]
fn a_denied_domain_reason_needs_words_and_a_key_without_an_entry_never_fires() -> TestResult {
    let empty = refused(&with_network_extra("deniedDomainReasons", r#"{"blocked.test": ""}"#))?;
    assert!(
        empty.path().starts_with("network.deniedDomainReasons"),
        "{}",
        empty.path()
    );
    let unmatched = network_options_of(r#""deniedDomainReasons": {"not a domain": "why"}"#)?;
    assert!(unmatched.denied_domain_reasons.is_empty());
    Ok(())
}

#[test]
fn strict_allowlist_refuses_unlisted_destinations() -> TestResult {
    assert_eq!(
        network_options_of(r#""strictAllowlist": true"#)?.unlisted,
        UnlistedDestinations::Refuse
    );
    assert_eq!(
        network_options_of(r#""strictAllowlist": false"#)?.unlisted,
        UnlistedDestinations::Consult
    );
    refused(&with_network_extra("strictAllowlist", r#""yes""#))?;
    Ok(())
}

#[test]
fn a_parent_proxy_is_built_from_the_explicit_urls() -> TestResult {
    let options =
        network_options_of(r#""parentProxy": {"http": "http://proxy.corp:3128", "noProxy": "internal.corp"}"#)?;
    let parent = options.parent_proxy.ok_or("no parent proxy")?;
    assert_eq!(parent.http().map(|url| url.port().get()), Some(3128));
    assert_eq!(parent.https().map(|url| url.port().get()), Some(3128));
    assert_eq!(parent.no_proxy().entries().len(), 1);
    Ok(())
}

#[test]
fn a_parent_proxy_falls_back_to_the_environment_only_when_the_key_is_present() -> TestResult {
    let env = OneVar("HTTPS_PROXY", "http://env.corp:8443");
    let without = network_options_with(r#""strictAllowlist": false"#, &env)?;
    assert!(without.parent_proxy.is_none());
    let with = network_options_with(r#""parentProxy": {}"#, &env)?;
    let parent = with.parent_proxy.ok_or("no parent proxy")?;
    assert_eq!(parent.https().map(|url| url.port().get()), Some(8443));
    assert!(parent.http().is_none());
    Ok(())
}

#[test]
fn a_parent_proxy_url_that_does_not_parse_is_refused() -> TestResult {
    for url in ["\"ftp://p:1\"", "\"http://\"", "7"] {
        let err = refused(&with_network_extra("parentProxy", &format!(r#"{{"http": {url}}}"#)))?;
        assert_eq!(err.path(), "network.parentProxy.http", "{url}");
    }
    Ok(())
}

#[test]
fn a_mitm_proxy_routes_its_domains_to_the_socket() -> TestResult {
    let options = network_options_of(
        r#""mitmProxy": {"socketPath": "/run/mitm.sock", "domains": ["api.example.com", "*.internal.org"]}"#,
    )?;
    let mitm = options.mitm.ok_or("no mitm proxy")?;
    assert_eq!(mitm.socket_path, PathBuf::from("/run/mitm.sock"));
    assert_eq!(
        mitm.domains,
        [
            DomainPattern::new("api.example.com")?,
            DomainPattern::new("*.internal.org")?
        ]
    );
    Ok(())
}

#[test]
fn a_mitm_proxy_needs_a_socket_and_a_domain_and_reads_a_relative_socket_from_the_cwd() -> TestResult {
    let relative = accepted(&with_network_extra(
        "mitmProxy",
        r#"{"socketPath": "mitm.sock", "domains": ["a.com"]}"#,
    ))?;
    let cwd = std::env::current_dir()?;
    let options = relative.network_options()?;
    assert_eq!(
        options.mitm.ok_or("a value was absent")?.socket_path,
        cwd.join("mitm.sock")
    );
    let empty = refused(&with_network_extra(
        "mitmProxy",
        r#"{"socketPath": "/run/mitm.sock", "domains": []}"#,
    ))?;
    assert_eq!(empty.path(), "network.mitmProxy.domains");
    let missing = refused(&with_network_extra("mitmProxy", r#"{"domains": ["a.com"]}"#))?;
    assert_eq!(missing.path(), "network.mitmProxy.socketPath");
    Ok(())
}

#[test]
fn tls_terminate_max_tunnels_is_the_tunnel_cap() -> TestResult {
    let options = network_options_of(r#""tlsTerminate": {"maxTunnels": 12}"#)?;
    assert_eq!(options.tunnel_cap, Some(TunnelCap::new(12)?));
    assert_eq!(network_options_of(r#""tlsTerminate": {}"#)?.tunnel_cap, None);
    for count in ["0", "65537", "-1", "1.5"] {
        let err = refused(&with_network_extra(
            "tlsTerminate",
            &format!(r#"{{"maxTunnels": {count}}}"#),
        ))?;
        assert_eq!(err.path(), "network.tlsTerminate.maxTunnels", "{count}");
    }
    Ok(())
}

#[test]
fn strip_response_headers_fold_to_one_list() -> TestResult {
    let options = network_options_of(r#""stripResponseHeaders": ["Set-Cookie", "x_powered_by", "set.cookie"]"#)?;
    let names: Vec<_> = options
        .strip_response_headers
        .headers()
        .iter()
        .map(|header| header.as_str().to_string())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.iter().any(|name| name == "set-cookie"), "{names:?}");
    let framing = network_options_of(
        r#""stripResponseHeaders": ["Content-Length", "transfer-encoding", "te", "trailer", "upgrade"]"#,
    )?;
    assert_eq!(framing.strip_response_headers.headers().len(), 5);
    for name in ["\"\"", "\"bad header\""] {
        let err = refused(&with_network_extra("stripResponseHeaders", &format!("[{name}]")))?;
        assert_eq!(err.path(), "network.stripResponseHeaders[0]", "{name}");
    }
    Ok(())
}

#[test]
fn macos_opt_ins_apply_on_macos_only() -> TestResult {
    let json = r#"{"network": {"allowedDomains": [], "deniedDomains": [],
                "allowLocalBinding": true, "allowMachLookup": ["com.example.*", "com.apple.x"]},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
            "allowPty": true, "enableWeakerNetworkIsolation": true}"#;
    let settings = accepted(json)?;
    let macos = settings.platform_options_for(true, false);
    assert!(macos.macos.allow_pty.is_allowed());
    assert!(macos.macos.allow_local_binding.is_allowed());
    assert!(macos.macos.allow_trust_service.is_allowed());
    let names: Vec<_> = macos.macos.allow_mach_lookup.iter().map(|n| n.as_str()).collect();
    assert_eq!(names, ["com.example.*", "com.apple.x"]);
    assert!(macos.linux.is_default());
    assert_eq!(settings.platform_options_for(false, true), PlatformOptions::default());
    Ok(())
}

#[test]
fn each_macos_opt_in_is_independent() -> TestResult {
    let only = |key: &str, network: bool| {
        let json = if network {
            with_network_extra(key, "true")
        } else {
            with_extra(key, "true")
        };
        Ok::<_, Box<dyn std::error::Error>>(accepted(&json)?.platform_options_for(true, false).macos)
    };
    let pty = only("allowPty", false)?;
    assert!(
        pty.allow_pty.is_allowed() && !pty.allow_local_binding.is_allowed() && !pty.allow_trust_service.is_allowed()
    );
    let binding = only("allowLocalBinding", true)?;
    assert!(binding.allow_local_binding.is_allowed() && !binding.allow_pty.is_allowed());
    let trust = only("enableWeakerNetworkIsolation", false)?;
    assert!(trust.allow_trust_service.is_allowed() && !trust.allow_pty.is_allowed());
    Ok(())
}

#[test]
fn a_mach_service_name_with_a_wildcard_inside_is_refused() -> TestResult {
    let err = refused(&with_network_extra("allowMachLookup", r#"["com.*.x"]"#))?;
    assert_eq!(err.path(), "network.allowMachLookup[0]");
    assert_eq!(
        err.message(),
        "Wildcards are only allowed as a single trailing \"*\" (e.g., \"com.example.*\" or \"*\" for all services)."
    );
    accepted(&with_network_extra("allowMachLookup", r#"[""]"#))?;
    Ok(())
}

#[test]
fn the_weaker_nested_sandbox_applies_on_linux_only() -> TestResult {
    let settings = accepted(&with_extra("enableWeakerNestedSandbox", "true"))?;
    assert!(
        settings
            .platform_options_for(false, true)
            .linux
            .weaker_nested_sandbox
            .is_weaker()
    );
    assert_eq!(settings.platform_options_for(true, false), PlatformOptions::default());
    refused(&with_extra("enableWeakerNestedSandbox", r#""yes""#))?;
    Ok(())
}

#[test]
fn git_safe_directories_apply_on_every_platform() -> TestResult {
    let settings = accepted(&with_extra(
        "git",
        r#"{"safeDirectories": ["/workspace/repo", "/workspace/./other/"]}"#,
    ))?;
    for (macos, linux) in [(true, false), (false, true)] {
        let dirs: Vec<_> = settings
            .platform_options_for(macos, linux)
            .git_safe_directories
            .iter()
            .map(|dir| dir.as_str().to_string())
            .collect();
        assert_eq!(dirs, ["/workspace/repo", "/workspace/other"]);
    }
    assert!(
        accepted(&with_extra("git", "{}"))?
            .platform_options()
            .git_safe_directories
            .is_empty()
    );
    let err = refused(&with_extra("git", r#"{"safeDirectories": ["relative"]}"#))?;
    assert_eq!(err.path(), "git.safeDirectories[0]");
    assert_eq!(
        refused(&with_extra("git", r#"{"other": 1}"#))?.to_string(),
        "git: Unrecognized key(s) in object: 'other'"
    );
    Ok(())
}

#[test]
fn into_config_carries_the_network_options_and_the_platform_opt_ins() -> TestResult {
    let dir = tempfile::tempdir()?;
    let json = r#"{"network": {"allowedDomains": [], "deniedDomains": ["blocked.test"],
            "strictAllowlist": true, "stripResponseHeaders": ["set-cookie"]},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
            "git": {"safeDirectories": ["/workspace"]}}"#;
    let config = accepted(json)?.into_config(dir.path())?;
    assert_eq!(
        config.configured_network_options().unlisted,
        UnlistedDestinations::Refuse
    );
    assert!(
        config
            .configured_network_options()
            .strip_response_headers
            .headers()
            .iter()
            .any(|header| header.as_str() == "set-cookie")
    );
    assert_eq!(config.configured_platform_options().git_safe_directories.len(), 1);
    Ok(())
}

fn with_credentials(allowed: &str, tls: bool, credentials: &str) -> String {
    let tls = if tls { r#", "tlsTerminate": {}"# } else { "" };
    format!(
        r#"{{"network": {{"allowedDomains": {allowed}, "deniedDomains": []{tls}}},
            "filesystem": {{"denyRead": [], "allowWrite": [], "denyWrite": []}},
            "credentials": {credentials}}}"#
    )
}

fn credential_config(
    allowed: &str,
    tls: bool,
    credentials: &str,
) -> Result<PolicyExecConfig, Box<dyn std::error::Error>> {
    Ok(accepted(&with_credentials(allowed, tls, credentials))?.into_config("/work")?)
}

fn credential_error(allowed: &str, tls: bool, credentials: &str) -> Result<SettingsError, Box<dyn std::error::Error>> {
    let json = with_credentials(allowed, tls, credentials);
    match SandboxSettings::from_json(&json) {
        Err(err) => Ok(err),
        Ok(settings) => Ok(settings
            .into_config("/work")
            .err()
            .ok_or("the credentials were accepted")?),
    }
}

#[test]
fn masked_env_var_defaults_to_the_allowed_domains() -> TestResult {
    let config = credential_config(
        r#"["api.example.com", "*.example.org:443"]"#,
        true,
        r#"{"envVars": [{"name": "API_TOKEN", "mode": "mask"}]}"#,
    )?;

    let binding = &config.bindings()[0];
    assert_eq!(binding.mode(), trogon_sandbox_contract::SecretMode::Mask);
    assert_eq!(binding.inject_hosts().hosts().len(), 2);
    assert!(binding.inject_hosts().hosts().iter().all(|host| host.port().is_none()));
    Ok(())
}

#[test]
fn a_command_scopes_a_masked_variable_to_its_own_allowed_domains() -> TestResult {
    let config = credential_config(
        r#"["api.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "API_TOKEN", "mode": "mask"}]}"#,
    )?;
    let context = CredentialContext::of(&config).ok_or("a value was absent")?;
    let own = [DomainPattern::from_str("other.example.org")?];

    let effective = context.effective(None, Some(&own))?;

    assert_eq!(effective.rules.bindings[0].inject_hosts().hosts(), &own);
    let session = context.effective(None, None)?;
    assert_eq!(session.rules.bindings[0].inject_hosts().hosts().len(), 1);
    assert_ne!(
        session.rules.bindings[0].inject_hosts().hosts(),
        effective.rules.bindings[0].inject_hosts().hosts()
    );
    Ok(())
}

#[test]
fn a_command_credentials_section_replaces_the_sessions_as_a_whole() -> TestResult {
    let config = credential_config(
        r#"["api.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "API_TOKEN", "mode": "mask"}], "files": [{"path": "/home/*/session-secret", "mode": "deny"}]}"#,
    )?;
    let context = CredentialContext::of(&config).ok_or("a value was absent")?;
    assert_eq!(context.session_deny().len(), 1);

    let own = CredentialOverride::from_json(r#"{"files": [{"path": "/home/me/.netrc", "mode": "deny"}]}"#)?;
    let effective = context.effective(Some(&own), None)?;

    assert_eq!(effective.rules.bindings.len(), 1);
    assert_eq!(
        effective.rules.bindings[0].mode(),
        trogon_sandbox_contract::SecretMode::Deny
    );
    assert!(effective.deny.is_empty());

    let kept = context.effective(None, None)?;
    assert_eq!(kept.rules.bindings.len(), 1);
    assert_eq!(kept.rules.bindings[0].name().as_str(), "env.API_TOKEN");
    assert_eq!(kept.deny, context.session_deny());
    Ok(())
}

#[test]
fn a_command_credentials_section_is_checked_as_the_sessions_is() -> TestResult {
    assert!(CredentialOverride::from_json(r#"{"foreign": 1}"#).is_err());
    let config = credential_config(r#"["a.example.com"]"#, false, "{}")?;
    let masking = CredentialOverride::from_json(r#"{"envVars": [{"name": "T", "mode": "mask"}]}"#)?;
    let error = CredentialContext::of(&config)
        .ok_or("a value was absent")?
        .effective(Some(&masking), None)
        .err()
        .ok_or("expected an error")?;
    assert!(error.message().contains("tlsTerminate"), "{error}");
    Ok(())
}

#[test]
fn masking_without_tls_termination_is_refused() -> TestResult {
    let err = credential_error(
        r#"["api.example.com"]"#,
        false,
        r#"{"envVars": [{"name": "API_TOKEN", "mode": "mask"}]}"#,
    )?;

    assert!(err.message().contains("tlsTerminate"), "{err}");
    Ok(())
}

#[test]
fn deny_mode_needs_no_tls_termination() -> TestResult {
    let config = credential_config(
        "[]",
        false,
        r#"{"files": [{"path": "/home/me/.netrc", "mode": "deny"}]}"#,
    )?;

    assert_eq!(config.bindings()[0].mode(), trogon_sandbox_contract::SecretMode::Deny);
    Ok(())
}

#[test]
fn inject_hosts_outside_the_allowed_domains_are_refused() -> TestResult {
    let err = credential_error(
        r#"["api.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "T", "mode": "mask", "injectHosts": ["other.example.net"]}]}"#,
    )?;

    assert_eq!(err.path(), "credentials.envVars[0].injectHosts[0]");
    Ok(())
}

#[test]
fn explicitly_empty_inject_hosts_are_refused() -> TestResult {
    let err = credential_error(
        r#"["api.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "T", "mode": "mask", "injectHosts": []}]}"#,
    )?;

    assert_eq!(err.path(), "credentials.envVars[0]");
    Ok(())
}

#[test]
fn plaintext_injection_is_off_unless_asked_for() -> TestResult {
    let mask = r#""envVars": [{"name": "API_TOKEN", "mode": "mask"}]"#;
    let off = credential_config(
        r#"["api.example.com"]"#,
        true,
        &format!(r#"{{{mask}, "allowPlaintextInject": false}}"#),
    )?;
    assert!(!off.allows_plaintext_inject());

    let on = credential_config(
        r#"["api.example.com"]"#,
        true,
        &format!(r#"{{{mask}, "allowPlaintextInject": true}}"#),
    )?;
    assert!(on.allows_plaintext_inject());
    Ok(())
}

#[test]
fn plaintext_injection_lifts_the_tls_terminate_requirement_for_masking() -> TestResult {
    let mask = r#""envVars": [{"name": "API_TOKEN", "mode": "mask"}]"#;
    let err = credential_error(r#"["api.example.com"]"#, false, &format!("{{{mask}}}"))?;
    assert_eq!(err.path(), "credentials");

    let config = credential_config(
        r#"["api.example.com"]"#,
        false,
        &format!(r#"{{{mask}, "allowPlaintextInject": true}}"#),
    )?;
    assert!(config.allows_plaintext_inject());
    assert_eq!(config.bindings().len(), 1);
    Ok(())
}

#[test]
fn unknown_credential_keys_are_refused() -> TestResult {
    let err = refused(&with_credentials(
        "[]",
        false,
        r#"{"envVars": [{"name": "T", "mode": "deny", "bogus": 1}]}"#,
    ))?;

    assert!(err.message().contains("bogus"), "{err}");
    Ok(())
}

#[test]
fn mask_claims_need_decode() -> TestResult {
    let err = credential_error(
        r#"["api.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "T", "mode": "mask", "maskClaims": ["sub"]}]}"#,
    )?;

    assert_eq!(err.path(), "credentials.envVars[0].maskClaims");
    Ok(())
}

#[test]
fn an_env_var_with_decode_verifies_its_whole_value_and_ignores_extract() -> TestResult {
    let config = credential_config(
        r#"["api.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "T", "mode": "mask", "extract": "token=(\\S+)", "decode": "jwt", "maskClaims": ["sub"]}]}"#,
    )?;

    let Some(trogon_sandbox_contract::Extract::Jwt(masking)) = config.bindings()[0].extract() else {
        return Err("expected a JWT rule".into());
    };
    assert!(masking.pattern.is_none());
    assert_eq!(masking.mask_claims.len(), 1);
    Ok(())
}

#[test]
fn identical_duplicate_env_vars_and_file_paths_collapse() -> TestResult {
    let config = credential_config(
        "[]",
        false,
        r#"{"envVars": [{"name": "T", "mode": "deny"}, {"name": "T", "mode": "deny"}],
            "files": [{"path": "/a", "mode": "deny"}, {"path": "/a", "mode": "deny"}]}"#,
    )?;
    assert_eq!(config.bindings().len(), 2);
    Ok(())
}

#[test]
fn duplicates_with_different_settings_are_refused() -> TestResult {
    let vars = credential_error(
        r#"["a.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "T", "mode": "deny"}, {"name": "T", "mode": "mask"}]}"#,
    )?;
    assert_eq!(vars.path(), "credentials.envVars[1].name");
    Ok(())
}

#[test]
fn a_file_listed_again_with_different_settings_keeps_the_last() -> TestResult {
    let config = credential_config(
        r#"["a.example.com"]"#,
        true,
        r#"{"files": [{"path": "/a", "mode": "deny"}, {"path": "/a", "mode": "mask"}]}"#,
    )?;
    assert_eq!(config.bindings().len(), 1);
    assert_eq!(config.bindings()[0].mode(), trogon_sandbox_contract::SecretMode::Mask);
    Ok(())
}

#[test]
fn credential_file_globs_are_refused() -> TestResult {
    let err = credential_error(
        "[]",
        true,
        r#"{"files": [{"path": "/home/*/.ssh/id_rsa", "mode": "mask"}]}"#,
    )?;
    assert_eq!(err.path(), "credentials.files[0].path");
    Ok(())
}

#[test]
fn masked_file_paths_must_name_a_file() -> TestResult {
    let relative = credential_error(
        r#"["api.example.com"]"#,
        true,
        r#"{"files": [{"path": "relative/key", "mode": "mask"}]}"#,
    )?;
    assert_eq!(relative.path(), "credentials.files[0].path");

    let directory = credential_error(
        r#"["api.example.com"]"#,
        true,
        r#"{"files": [{"path": "/etc/keys/", "mode": "mask"}]}"#,
    )?;
    assert_eq!(directory.path(), "credentials.files[0].path");
    Ok(())
}

#[test]
fn aws_pairs_name_masked_env_vars() -> TestResult {
    let config = credential_config(
        r#"["s3.amazonaws.com"]"#,
        true,
        r#"{"envVars": [
              {"name": "MY_KEY_ID", "mode": "mask"},
              {"name": "MY_SECRET", "mode": "mask"}],
            "awsPairs": [{"accessKeyIdVar": "MY_KEY_ID", "secretAccessKeyVar": "MY_SECRET"}],
            "sigv4": {"streaming": "passthrough"}}"#,
    )?;

    assert_eq!(config.aws_pairs().len(), 1);
    assert_eq!(
        config.configured_sigv4_policies().streaming,
        trogon_sandbox_egress_proxy::UnsupportedSigv4Policy::Passthrough
    );
    Ok(())
}

#[test]
fn aws_pairs_must_reference_masked_env_vars() -> TestResult {
    let err = credential_error(
        r#"["s3.amazonaws.com"]"#,
        true,
        r#"{"envVars": [{"name": "MY_KEY_ID", "mode": "mask"}],
            "awsPairs": [{"accessKeyIdVar": "MY_KEY_ID", "secretAccessKeyVar": "MISSING"}]}"#,
    )?;

    assert_eq!(err.path(), "credentials.awsPairs[0].secretAccessKeyVar");
    Ok(())
}

#[test]
fn explicit_aws_pairs_link_non_standard_names_including_the_session_token() -> TestResult {
    let config = credential_config(
        r#"["s3.amazonaws.com"]"#,
        true,
        r#"{"envVars": [
              {"name": "MY_KEY_ID", "mode": "mask"},
              {"name": "MY_SECRET", "mode": "mask"},
              {"name": "MY_TOKEN", "mode": "mask"}],
            "awsPairs": [{"accessKeyIdVar": "MY_KEY_ID", "secretAccessKeyVar": "MY_SECRET", "sessionTokenVar": "MY_TOKEN"}]}"#,
    )?;

    let pairs = config.aws_pairs();
    assert_eq!(pairs.len(), 1);
    let name_of = |var: &str| -> Result<trogon_sandbox_contract::SecretName, Box<dyn std::error::Error>> {
        Ok(config
            .bindings()
            .iter()
            .find(|binding| {
                matches!(binding.target(), trogon_sandbox_contract::SecretTarget::Env(name) if name.as_str() == var)
            })
            .ok_or("a value was absent")?
            .name()
            .clone())
    };
    assert_eq!(pairs[0].access_key_id(), &name_of("MY_KEY_ID")?);
    assert_eq!(pairs[0].secret_access_key(), &name_of("MY_SECRET")?);
    assert_eq!(pairs[0].session_token(), Some(&name_of("MY_TOKEN")?));
    Ok(())
}

#[test]
fn a_denied_variable_is_not_a_valid_aws_pair_member() -> TestResult {
    let err = credential_error(
        r#"["s3.amazonaws.com"]"#,
        true,
        r#"{"envVars": [
              {"name": "MY_KEY_ID", "mode": "mask"},
              {"name": "MY_SECRET", "mode": "deny"}],
            "awsPairs": [{"accessKeyIdVar": "MY_KEY_ID", "secretAccessKeyVar": "MY_SECRET"}]}"#,
    )?;

    assert_eq!(err.path(), "credentials.awsPairs[0].secretAccessKeyVar");
    Ok(())
}

#[test]
fn an_extracted_variable_is_not_a_valid_aws_pair_member() -> TestResult {
    let err = credential_error(
        r#"["s3.amazonaws.com"]"#,
        true,
        r#"{"envVars": [
              {"name": "MY_KEY_ID", "mode": "mask"},
              {"name": "MY_SECRET", "mode": "mask", "extract": "secret=(\\S+)"}],
            "awsPairs": [{"accessKeyIdVar": "MY_KEY_ID", "secretAccessKeyVar": "MY_SECRET"}]}"#,
    )?;

    assert_eq!(err.path(), "credentials.awsPairs[0].secretAccessKeyVar");
    assert!(err.message().contains("whole-value"), "{err}");
    Ok(())
}

#[test]
fn a_variable_cannot_belong_to_two_aws_pairs() -> TestResult {
    let err = credential_error(
        r#"["s3.amazonaws.com"]"#,
        true,
        r#"{"envVars": [
              {"name": "ID_ONE", "mode": "mask"},
              {"name": "SECRET_ONE", "mode": "mask"},
              {"name": "SECRET_TWO", "mode": "mask"}],
            "awsPairs": [
              {"accessKeyIdVar": "ID_ONE", "secretAccessKeyVar": "SECRET_ONE"},
              {"accessKeyIdVar": "ID_ONE", "secretAccessKeyVar": "SECRET_TWO"}]}"#,
    )?;

    assert_eq!(err.path(), "credentials.awsPairs[1].accessKeyIdVar");
    Ok(())
}

#[test]
fn sigv4_rejects_unknown_policies_and_unknown_keys() -> TestResult {
    for sigv4 in [
        r#"{"streaming": "allow"}"#,
        r#"{"presigned": "bogus"}"#,
        r#"{"sigv4a": 1}"#,
        r#"{"unknownShape": "deny"}"#,
    ] {
        let json = with_credentials(r#"["s3.amazonaws.com"]"#, true, &format!(r#"{{"sigv4": {sigv4}}}"#));
        SandboxSettings::from_json(&json).err().ok_or(sigv4)?;
    }
    Ok(())
}

#[test]
fn tls_terminate_terminates_every_allowed_host_and_its_absence_none() -> TestResult {
    let terminating = accepted(&with_network_extra("tlsTerminate", "{}"))?;
    assert!(terminating.network_options()?.terminate_allowed_tls.is_enabled());

    let opaque = accepted(&with_network_extra("allowLocalBinding", "false"))?;
    assert!(!opaque.network_options()?.terminate_allowed_tls.is_enabled());
    Ok(())
}

#[test]
fn extra_ca_cert_paths_reach_the_network_options() -> TestResult {
    let settings = accepted(&with_network_extra(
        "tlsTerminate",
        r#"{"extraCaCertPaths": ["/etc/corp-ca.pem"]}"#,
    ))?;

    let options = settings.network_options()?;

    assert_eq!(
        options.extra_ca_cert_paths.as_slice(),
        [PathBuf::from("/etc/corp-ca.pem")]
    );
    Ok(())
}

#[test]
fn relative_extra_ca_cert_paths_are_read_from_the_working_directory() -> TestResult {
    let settings = accepted(&with_network_extra(
        "tlsTerminate",
        r#"{"extraCaCertPaths": ["ca.pem"]}"#,
    ))?;
    let cwd = std::env::current_dir()?;

    let options = settings.network_options()?;

    assert_eq!(options.extra_ca_cert_paths.as_slice(), [cwd.join("ca.pem")]);
    assert!(
        refused(&with_network_extra("tlsTerminate", r#"{"extraCaCertPaths": [""]}"#,))?
            .path()
            .starts_with("network.tlsTerminate.extraCaCertPaths")
    );
    Ok(())
}

#[test]
fn tool_and_proxy_keys_that_match_this_runtime_load() -> TestResult {
    for (key, value) in [
        ("allowPlaintextHeaderSet", "false"),
        ("requireHostMatch", "true"),
        ("requireHostMatch", "false"),
        ("ambiguousRequestTarget", r#""refuse""#),
        ("tlsTerminate", r#"{"excludeDomains": []}"#),
        ("tlsTerminate", r#"{"handshakeTimeoutMs": 10000}"#),
    ] {
        accepted(&with_network_extra(key, value))?;
    }
    for (key, value) in [
        ("ripgrep", r#"{"command": "rg", "args": ["--hidden"], "argv0": "rg"}"#),
        ("seccomp", r#"{"applyPath": "/x/apply", "argv0": "apply"}"#),
        ("seccomp", "{}"),
        ("mandatoryDenySearchDepth", "1"),
        ("mandatoryDenySearchDepth", "10"),
    ] {
        accepted(&with_extra(key, value))?;
    }
    Ok(())
}

fn host_paths() -> HostPaths {
    HostPaths::new("/work/project").with_home("/home/me")
}

fn host_policy(allow_write: &str) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(
        SandboxSettings::from_json_for_host(&with_fs("[]", allow_write, "[]"), &host_paths())?
            .policy()?
            .fs
            .ok_or("no filesystem policy")?,
    )
}

fn literal_paths(entries: &[FsPathEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry {
            FsPathEntry::Literal { path, .. } => path.to_string(),
            FsPathEntry::Glob { pattern } => pattern.to_string(),
        })
        .collect()
}

#[test]
fn host_paths_expand_home_and_the_working_directory() -> TestResult {
    let fs = host_policy(r#"["~", "~/.cache", ".", "./out", "build", "/abs//x/./y/"]"#)?;
    assert_eq!(
        literal_paths(fs.allow_write()),
        [
            "/home/me",
            "/home/me/.cache",
            "/work/project",
            "/work/project/out",
            "/work/project/build",
            "/abs/x/y"
        ]
    );
    Ok(())
}

#[test]
fn host_globs_keep_their_wildcards() -> TestResult {
    let fs = host_policy(r#"["src/**/*.rs", "~/.aws//*.pem"]"#)?;
    assert_eq!(
        literal_paths(fs.allow_write()),
        ["/work/project/src/**/*.rs", "/home/me/.aws/*.pem"]
    );
    Ok(())
}

#[cfg(unix)]
fn link_named(root: &std::path::Path, target: &str) -> Result<String, Box<dyn std::error::Error>> {
    let link = root.join("agent");
    std::os::unix::fs::symlink(target, &link)?;
    Ok(link.display().to_string())
}

#[cfg(unix)]
#[test]
fn an_allow_whose_name_is_a_link_to_the_root_keeps_its_name() -> TestResult {
    let (_guard, root) = real_dir()?;
    let name = link_named(&root, "/")?;
    let fs = host_fs("[]", &format!(r#"["{name}"]"#), "[]", &host_paths())?;
    assert_eq!(literal_paths(fs.allow_write()), [name]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_allow_whose_name_is_a_link_to_its_parent_keeps_its_name() -> TestResult {
    let (_guard, root) = real_dir()?;
    let name = link_named(&root, root.to_str().ok_or("a value was absent")?)?;
    let fs = host_fs("[]", &format!(r#"["{name}"]"#), "[]", &host_paths())?;
    assert_eq!(literal_paths(fs.allow_write()), [name]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_allow_whose_name_is_a_link_to_an_unrelated_path_keeps_its_name() -> TestResult {
    let (_guard, root) = real_dir()?;
    std::fs::create_dir(root.join("elsewhere"))?;
    let name = link_named(&root, root.join("elsewhere").to_str().ok_or("a value was absent")?)?;
    let fs = host_fs("[]", &format!(r#"["{name}"]"#), "[]", &host_paths())?;
    assert_eq!(literal_paths(fs.allow_write()), [name]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_deny_glob_whose_base_is_a_link_to_the_root_keeps_its_pattern() -> TestResult {
    let (_guard, root) = real_dir()?;
    let name = link_named(&root, "/")?;
    let fs = host_fs(&format!(r#"["{name}/**"]"#), "[]", "[]", &host_paths())?;
    assert!(
        literal_paths(fs.deny_read()).iter().all(|path| path.starts_with(&name)),
        "{:?}",
        literal_paths(fs.deny_read())
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_deny_glob_whose_base_is_a_link_to_its_parent_keeps_its_pattern() -> TestResult {
    let (_guard, root) = real_dir()?;
    let name = link_named(&root, root.to_str().ok_or("a value was absent")?)?;
    let fs = host_fs(&format!(r#"["{name}/*.pem"]"#), "[]", "[]", &host_paths())?;
    assert_eq!(literal_paths(fs.deny_read()), [format!("{name}/*.pem")]);
    Ok(())
}

#[test]
fn an_empty_host_filesystem_reads_everything_and_writes_nothing() -> TestResult {
    let settings = SandboxSettings::from_json_for_host(&with_fs("[]", "[]", "[]"), &host_paths())?;
    let fs = settings.policy()?.fs.ok_or("no filesystem policy")?;
    assert!(fs.is_empty());
    let config = settings.into_config("/work/project")?;
    assert_eq!(config.filesystem_view(), FilesystemView::Host);
    Ok(())
}

#[test]
fn the_guest_view_still_refuses_relative_and_home_paths_with_their_path() -> TestResult {
    let err = refused(&with_fs("[]", r#"["ok", "~/x"]"#, "[]"))?;
    assert_eq!(err.path(), "filesystem.allowWrite[0]");
    let settings = accepted(&with_fs("[]", "[]", "[]"))?;
    assert!(settings.policy()?.fs.is_none());
    Ok(())
}

#[test]
fn host_paths_report_the_entry_that_is_wrong() -> TestResult {
    let err = SandboxSettings::from_json_for_host(&with_fs("[]", r#"["a", ""]"#, "[]"), &host_paths())
        .err()
        .ok_or("empty")?;
    assert_eq!(err.path(), "filesystem.allowWrite[1]");
    Ok(())
}

fn host_fs(
    deny_read: &str,
    allow_write: &str,
    deny_write: &str,
    host: &HostPaths,
) -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(
        SandboxSettings::from_json_for_host(&with_fs(deny_read, allow_write, deny_write), host)?
            .policy()?
            .fs
            .ok_or("no filesystem policy")?,
    )
}

fn real_dir() -> Result<(tempfile::TempDir, std::path::PathBuf), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    Ok((dir, root))
}

#[test]
fn a_dir_globstar_suffix_covers_the_directory_itself() -> TestResult {
    let fs = host_policy(r#"["/data/**", "/"]"#)?;
    assert_eq!(literal_paths(fs.allow_write()), ["/data", "/"]);
    assert!(
        fs.allow_write()
            .iter()
            .all(|entry| matches!(entry, FsPathEntry::Literal { .. }))
    );
    Ok(())
}

#[test]
fn a_deny_with_glob_characters_also_denies_the_directory_of_that_name() -> TestResult {
    let (_guard, root) = real_dir()?;
    let named = root.join("secret[1]");
    std::fs::create_dir(&named)?;
    let spelling = format!("{}/secret[1]", root.display());
    let fs = host_fs(&format!(r#"["{spelling}"]"#), "[]", "[]", &host_paths())?;
    let paths = literal_paths(fs.deny_read());
    assert!(paths.contains(&named.display().to_string()), "{paths:?}");
    assert!(fs.deny_read().iter().any(|e| matches!(e, FsPathEntry::Glob { .. })));
    Ok(())
}

#[test]
fn a_deny_with_glob_characters_stays_a_glob_when_no_such_name_exists() -> TestResult {
    let (_guard, root) = real_dir()?;
    let spelling = format!("{}/secret[1]", root.display());
    let fs = host_fs(&format!(r#"["{spelling}"]"#), "[]", "[]", &host_paths())?;
    assert!(fs.deny_read().iter().all(|e| matches!(e, FsPathEntry::Glob { .. })));
    Ok(())
}

#[test]
fn a_deny_glob_beneath_a_directory_whose_name_has_glob_characters_is_anchored_there() -> TestResult {
    let (_guard, root) = real_dir()?;
    std::fs::create_dir(root.join("a[b]"))?;
    let spelling = format!("{}/a[b]/*.pem", root.display());
    let fs = host_fs(&format!(r#"["{spelling}"]"#), "[]", "[]", &host_paths())?;
    let globs = fs
        .deny_read()
        .iter()
        .filter(|e| matches!(e, FsPathEntry::Glob { .. }))
        .count();
    assert!(globs >= 2, "{:?}", literal_paths(fs.deny_read()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_allow_does_not_take_a_name_reading_through_a_symlink() -> TestResult {
    let (_guard, root) = real_dir()?;
    let target = root.join("target");
    std::fs::create_dir(&target)?;
    std::os::unix::fs::symlink(&target, root.join("l[1]"))?;
    let spelling = format!("{}/l[1]", root.display());
    let allow = host_fs("[]", &format!(r#"["{spelling}"]"#), "[]", &host_paths())?;
    assert!(!literal_paths(allow.allow_write()).contains(&target.display().to_string()));
    let deny = host_fs(&format!(r#"["{spelling}"]"#), "[]", "[]", &host_paths())?;
    assert!(
        literal_paths(deny.deny_read())
            .iter()
            .any(|p| p.ends_with("l[1]") || p == &target.display().to_string())
    );
    Ok(())
}

#[test]
fn a_relative_deny_glob_with_a_trailing_slash_is_accepted() -> TestResult {
    accepted_for_host(&with_fs(r#"["build/*/"]"#, "[]", "[]"))?;
    Ok(())
}

fn accepted_for_host(json: &str) -> Result<SandboxSettings, Box<dyn std::error::Error>> {
    SandboxSettings::from_json_for_host(json, &host_paths()).map_err(|err| format!("{json} was refused: {err}").into())
}

#[test]
fn brackets_that_spell_no_set_are_warned_about() -> TestResult {
    let settings = accepted_for_host(&with_fs(r#"["/x/a[]b", "/x/ok"]"#, "[]", "[]"))?;
    assert_eq!(
        settings.warnings(),
        ["The brackets in [] spell no set of characters, so they are read as text: /x/a[]b"]
    );
    Ok(())
}

#[test]
fn a_parent_component_that_cannot_be_resolved_is_warned_about() -> TestResult {
    let settings = accepted_for_host(&with_fs(r#"["/nowhere-at-all/../x/*.pem"]"#, "[]", "[]"))?;
    assert!(
        settings
            .warnings()
            .iter()
            .any(|w| w.contains("still contains a \"..\" component")),
        "{:?}",
        settings.warnings()
    );
    Ok(())
}

#[test]
fn a_deny_mode_credential_file_with_a_glob_becomes_a_read_denial() -> TestResult {
    let json = with_extra(
        "credentials",
        r#"{"files": [{"path": "/x/keys/*.pem", "mode": "deny"}]}"#,
    );
    let settings = SandboxSettings::from_json_for_host(&json, &host_paths());
    let fs = settings?.policy()?.fs.ok_or("no filesystem policy")?;
    assert!(
        fs.credential_deny_read()
            .iter()
            .any(|e| matches!(e, FsPathEntry::Glob { .. }))
    );
    assert!(fs.deny_read().is_empty());
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn drop_linux_write_globs_drops_write_globs_with_a_warning() -> TestResult {
    let json = with_extra("dropLinuxWriteGlobs", "true");
    let json = json.replace(r#""allowWrite": []"#, r#""allowWrite": ["/w/*.log", "/w/ok"]"#);
    let settings = accepted_for_host(&json)?;
    let fs = settings.policy()?.fs.ok_or("no filesystem policy")?;
    assert_eq!(literal_paths(fs.allow_write()), ["/w/ok"]);
    assert_eq!(settings.warnings(), ["Skipping glob write pattern on Linux: /w/*.log"]);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn a_read_glob_with_no_directory_to_start_from_warns_on_linux() -> TestResult {
    let json = with_fs(r#"["/**/*.pem", "/opt*/keys/**", "/tmp/test/*.env"]"#, "[]", "[]");
    let settings = accepted_for_host(&json)?;
    assert_eq!(
        settings.warnings(),
        [
            "Skipping glob read pattern on Linux, it has no directory to start from: /**/*.pem",
            "Skipping glob read pattern on Linux, it has no directory to start from: /opt*/keys",
        ]
    );
    Ok(())
}

#[test]
fn drop_linux_write_globs_is_an_accepted_boolean_that_defaults_to_off() -> TestResult {
    accepted(&with_extra("dropLinuxWriteGlobs", "true"))?;
    accepted(&with_extra("dropLinuxWriteGlobs", "false"))?;
    refused(&with_extra("dropLinuxWriteGlobs", r#""yes""#))?;
    Ok(())
}

#[test]
fn a_document_without_the_network_key_is_refused_so_no_restriction_is_silently_skipped() -> TestResult {
    let err = refused(r#"{"filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []}}"#)?;
    assert!(err.path().starts_with("network"), "{}", err.path());
    Ok(())
}

#[test]
fn the_mandatory_deny_search_depth_reaches_the_linux_options_only() -> TestResult {
    let settings = accepted(&with_extra("mandatoryDenySearchDepth", "2"))?;
    let depth = |options: PlatformOptions| options.filesystem.mandatory_deny_search_depth;
    assert_eq!(
        depth(settings.platform_options_for(false, true)),
        Some(MandatoryDenySearchDepth::new(2)?)
    );
    assert_eq!(depth(settings.platform_options_for(true, false)), None);
    let unset = accepted(&with_extra("dropLinuxWriteGlobs", "false"))?;
    assert_eq!(depth(unset.platform_options_for(false, true)), None);
    Ok(())
}

fn with_excluded_credentials(allowed: &str, excluded: &str, credentials: &str) -> String {
    format!(
        r#"{{"network": {{"allowedDomains": {allowed}, "deniedDomains": [],
                "tlsTerminate": {{"excludeDomains": {excluded}}}}},
            "filesystem": {{"denyRead": [], "allowWrite": [], "denyWrite": []}},
            "credentials": {credentials}}}"#
    )
}

fn excluded_outcome(allowed: &str, excluded: &str, credentials: &str) -> Result<PolicyExecConfig, SettingsError> {
    SandboxSettings::from_json(&with_excluded_credentials(allowed, excluded, credentials))?.into_config("/work")
}

#[test]
fn a_masked_credential_whose_inject_host_is_fully_excluded_is_refused() -> TestResult {
    let err = excluded_outcome(
        r#"["*.example.com"]"#,
        r#"["*.internal.example.com"]"#,
        r#"{"envVars": [{"name": "TOKEN", "mode": "mask", "injectHosts": ["api.internal.example.com"]}]}"#,
    )
    .err()
    .ok_or("refused")?;

    assert_eq!(err.path(), "credentials.envVars[0].injectHosts[0]");
    assert!(err.message().contains("excludeDomains"), "{err}");
    assert!(err.message().contains("api.internal.example.com"), "{err}");
    Ok(())
}

#[test]
fn a_wildcard_inject_host_that_only_overlaps_the_excluded_domains_is_accepted() -> TestResult {
    excluded_outcome(
        r#"["*.example.com"]"#,
        r#"["mtls.example.com"]"#,
        r#"{"envVars": [{"name": "TOKEN", "mode": "mask", "injectHosts": ["*.example.com"]}]}"#,
    )?;
    Ok(())
}

#[test]
fn a_masked_credential_with_default_inject_hosts_is_refused_when_all_allowed_domains_are_excluded() -> TestResult {
    let err = excluded_outcome(
        r#"["*.example.com", "pinned.example.net"]"#,
        r#"["*.example.com", "pinned.example.net"]"#,
        r#"{"envVars": [{"name": "TOKEN", "mode": "mask"}]}"#,
    )
    .err()
    .ok_or("refused")?;

    assert_eq!(err.path(), "credentials.envVars[0]");
    assert!(err.message().contains("never be injected"), "{err}");
    Ok(())
}

#[test]
fn a_masked_credential_with_default_inject_hosts_coexists_with_a_partial_exclusion() -> TestResult {
    excluded_outcome(
        r#"["*.example.com"]"#,
        r#"["mtls.example.com"]"#,
        r#"{"envVars": [{"name": "TOKEN", "mode": "mask"}]}"#,
    )?;
    Ok(())
}

#[test]
fn inject_hosts_disjoint_from_the_excluded_domains_are_accepted() -> TestResult {
    excluded_outcome(
        r#"["*.example.com"]"#,
        r#"["mtls.example.com"]"#,
        r#"{"envVars": [{"name": "TOKEN", "mode": "mask", "injectHosts": ["api.example.com"]}]}"#,
    )?;
    Ok(())
}

#[test]
fn a_bare_star_or_a_scheme_in_the_excluded_domains_is_refused() -> TestResult {
    for excluded in [r#"["*"]"#, r#"["https://example.com"]"#] {
        let outcome = excluded_outcome("[]", excluded, r#"{}"#);
        assert!(outcome.is_err(), "{excluded}");
    }
    Ok(())
}

#[test]
fn mask_claims_must_name_at_least_one_claim_and_no_empty_one() -> TestResult {
    for claims in ["[]", r#"[""]"#] {
        for entry in [
            format!(r#"{{"envVars": [{{"name": "T", "mode": "mask", "decode": "jwt", "maskClaims": {claims}}}]}}"#),
            format!(
                r#"{{"files": [{{"path": "/home/me/.config/app/credentials", "mode": "mask", "decode": "jwt", "maskClaims": {claims}}}]}}"#
            ),
        ] {
            let outcome = SandboxSettings::from_json(&with_credentials(r#"["api.example.com"]"#, true, &entry))
                .and_then(|settings| settings.into_config("/work"));
            assert!(outcome.is_err(), "{entry}");
        }
    }
    Ok(())
}

#[test]
fn mask_claims_with_decode_on_a_deny_mode_entry_are_accepted_and_ignored() -> TestResult {
    credential_config(
        r#"["api.example.com"]"#,
        false,
        r#"{"envVars": [{"name": "T", "mode": "deny", "decode": "jwt", "maskClaims": ["api_key"]}]}"#,
    )?;
    Ok(())
}

#[test]
fn env_var_names_that_could_pass_for_options_or_assignments_are_refused() -> TestResult {
    for name in ["", "FOO=BAR", "FOO BAR", "--bind", "-u", "1FOO", "FOO.BAR"] {
        let json = with_credentials(
            "[]",
            false,
            &format!(r#"{{"envVars": [{{"name": "{name}", "mode": "deny"}}]}}"#),
        );
        let outcome = SandboxSettings::from_json(&json).and_then(|s| s.into_config("/work"));
        assert!(outcome.is_err(), "{name:?}");
    }
    Ok(())
}

#[test]
fn a_denied_credential_pattern_is_a_credential_deny_path() -> TestResult {
    let settings = accepted(&with_credentials(
        r#"["api.example.com"]"#,
        false,
        r#"{"files": [{"path": "/home/me/*.key", "mode": "deny"}]}"#,
    ))?;
    assert_eq!(settings.credential_deny_paths().len(), 1);
    assert!(accepted(&with_fs("[]", "[]", "[]"))?.credential_deny_paths().is_empty());
    Ok(())
}

const MASK_FILE: &str = "/home/me/.config/app/credentials";
const MASK_HOSTS: &str = r#"["api.example.com", "other.example.com"]"#;

fn credential_entries(extra: &str) -> [String; 2] {
    [
        format!(r#"{{"files": [{{"path": "{MASK_FILE}", "mode": "mask"{extra}}}]}}"#),
        format!(r#"{{"envVars": [{{"name": "T", "mode": "mask"{extra}}}]}}"#),
    ]
}

fn credential_entry_outcome(entry: &str) -> Result<PolicyExecConfig, SettingsError> {
    SandboxSettings::from_json(&with_credentials(MASK_HOSTS, true, entry))
        .and_then(|settings| settings.into_config("/work"))
}

#[test]
fn a_masked_entry_accepts_each_documented_option() -> TestResult {
    for extra in [
        r#", "injectHosts": ["api.example.com"]"#,
        r#", "extract": "token=(\\w+)""#,
        r#", "decode": "jwt""#,
        r#", "decode": "jwt", "maskClaims": ["sub"]"#,
        r#", "extract": "token=(\\w+)", "onExtractNoMatch": "warn""#,
        r#", "extract": "token=(\\w+)", "onExtractNoMatch": "deny""#,
        r#", "extract": "token=(\\w+)", "onExtractNoMatch": "error""#,
    ] {
        for entry in credential_entries(extra) {
            credential_entry_outcome(&entry).map_err(|err| format!("{entry}: {err}"))?;
        }
    }
    Ok(())
}

#[test]
fn a_masked_file_accepts_mask_duplicates_with_or_without_extract() -> TestResult {
    for extra in [
        r#", "extract": "token=(\\w+)", "maskDuplicates": true"#,
        r#", "maskDuplicates": true"#,
    ] {
        let [file, _] = credential_entries(extra);
        credential_entry_outcome(&file).map_err(|err| format!("{file}: {err}"))?;
    }
    Ok(())
}

#[test]
fn a_masked_entry_refuses_a_bad_option() -> TestResult {
    for extra in [
        r#", "decode": "base64""#,
        r#", "decode": """#,
        r#", "onExtractNoMatch": "ignore""#,
        r#", "injectHosts": []"#,
    ] {
        for entry in credential_entries(extra) {
            assert!(credential_entry_outcome(&entry).is_err(), "{entry}");
        }
    }
    Ok(())
}

#[test]
fn a_non_boolean_mask_duplicates_is_refused() -> TestResult {
    let [file, _] = credential_entries(r#", "maskDuplicates": "yes""#);
    assert!(credential_entry_outcome(&file).is_err());
    Ok(())
}

#[test]
fn an_unknown_credential_mode_is_refused() -> TestResult {
    for mode in ["allow", "block", ""] {
        for entry in [
            format!(r#"{{"files": [{{"path": "{MASK_FILE}", "mode": "{mode}"}}]}}"#),
            format!(r#"{{"envVars": [{{"name": "T", "mode": "{mode}"}}]}}"#),
        ] {
            assert!(credential_entry_outcome(&entry).is_err(), "{entry}");
        }
    }
    Ok(())
}

#[test]
fn an_empty_credential_file_path_is_refused() -> TestResult {
    let err = refused(&with_credentials(
        MASK_HOSTS,
        true,
        r#"{"files": [{"path": "", "mode": "mask"}]}"#,
    ))?;
    assert_eq!(err.path(), "credentials.files[0].path");
    Ok(())
}

#[test]
fn the_literal_mark_is_not_taken_on_a_credential_file() -> TestResult {
    let outcome = credential_entry_outcome(&format!(
        r#"{{"files": [{{"path": "{MASK_FILE}", "mode": "mask", "literal": true}}]}}"#
    ));
    assert!(outcome.is_err());
    Ok(())
}

#[test]
fn an_empty_credentials_object_loads_as_no_credential_policy() -> TestResult {
    let config = credential_config("[]", false, "{}")?;
    assert!(config.bindings().is_empty());
    Ok(())
}

#[test]
fn a_masked_file_loads_with_tls_termination_and_binds_one_secret() -> TestResult {
    let config = credential_config(
        MASK_HOSTS,
        true,
        &format!(r#"{{"files": [{{"path": "{MASK_FILE}", "mode": "mask"}}]}}"#),
    )?;
    assert_eq!(config.bindings().len(), 1);
    Ok(())
}

#[test]
fn a_deny_mode_entry_ignores_mask_duplicates() -> TestResult {
    for entry in [
        format!(r#"{{"files": [{{"path": "{MASK_FILE}", "mode": "deny", "maskDuplicates": true}}]}}"#),
        r#"{"envVars": [{"name": "T", "mode": "deny", "decode": "jwt", "maskClaims": ["sub"]}]}"#.to_string(),
    ] {
        credential_entry_outcome(&entry).map_err(|err| format!("{entry}: {err}"))?;
    }
    Ok(())
}

#[test]
fn a_slashed_glob_on_a_deny_mode_credential_file_is_refused() -> TestResult {
    let outcome = credential_entry_outcome(r#"{"files": [{"path": "/home/me/.aws/*/", "mode": "deny"}]}"#);
    assert!(outcome.is_err());
    Ok(())
}

#[test]
fn null_is_refused_on_every_optional_key_like_an_absent_only_key() -> TestResult {
    for (key, scope) in [
        ("ambiguousRequestTarget", "network"),
        ("allowLocalBinding", "network"),
        ("tlsTerminate", "network"),
    ] {
        let json = document(
            &format!(r#"{{"allowedDomains": [], "deniedDomains": [], "{key}": null}}"#),
            r#"{"denyRead": [], "allowWrite": [], "denyWrite": []}"#,
        );
        let err = refused(&json)?;
        assert!(err.path().starts_with(scope), "{key}: {}", err.path());
    }
    refused(&with_network_extra("tlsTerminate", r#"{"maxTunnels": null}"#))?;
    refused(&with_network_extra("tlsTerminate", r#"{"handshakeTimeoutMs": null}"#))?;
    refused(&with_extra("allowNestedUserNamespaces", "null"))?;
    refused(&with_extra("allowNestedUserNamespaces", r#""yes""#))?;
    Ok(())
}

#[test]
fn tunnel_and_handshake_bounds_are_inclusive() -> TestResult {
    for (key, ok, bad) in [
        ("maxTunnels", ["1", "65536"], ["0", "65537"]),
        ("handshakeTimeoutMs", ["100", "600000"], ["99", "600001"]),
    ] {
        for value in ok {
            accepted(&with_network_extra("tlsTerminate", &format!(r#"{{"{key}": {value}}}"#)))?;
        }
        for value in bad {
            refused(&with_network_extra("tlsTerminate", &format!(r#"{{"{key}": {value}}}"#)))?;
        }
    }
    Ok(())
}

#[test]
fn the_ambiguous_request_target_value_is_exact() -> TestResult {
    for bad in [r#""asspelled""#, r#""""#, "true", "1"] {
        refused(&with_network_extra("ambiguousRequestTarget", bad))?;
    }
    Ok(())
}

#[test]
fn allow_local_binding_takes_only_a_boolean() -> TestResult {
    for bad in [r#""all""#, "1", "[3000, 3099]"] {
        refused(&with_network_extra("allowLocalBinding", bad))?;
    }
    accepted(&with_network_extra("allowLocalBinding", "true"))?;
    Ok(())
}

#[test]
fn the_windows_sandbox_user_is_one_to_twenty_characters() -> TestResult {
    let document = |user: &str| with_extra("windows", &format!(r#"{{"sandboxUser": "{user}"}}"#));
    accepted(&document("sb-custom"))?.windows()?;
    accepted(&document(&"a".repeat(20)))?.windows()?;
    for bad in ["a".repeat(21), String::new()] {
        let err = refused(&document(&bad))?;
        assert_eq!(err.path(), "windows.sandboxUser");
    }
    Ok(())
}

#[test]
fn windows_keys_this_runtime_does_not_own_are_ignored() -> TestResult {
    accepted(&with_extra(
        "windows",
        r#"{"loopbackPortRange": [1, 2], "writeRestrictedToken": true, "proxyPortRange": [49152, 49200]}"#,
    ))?
    .windows()?;
    Ok(())
}

#[test]
fn an_inject_host_without_a_port_is_reachable_through_a_port_scoped_allow() -> TestResult {
    let credentials = r#"{"envVars": [{"name": "TOKEN", "mode": "mask", "injectHosts": ["api.example.com"]}]}"#;
    accepted(&with_credentials(r#"["api.example.com:443"]"#, true, credentials))?.into_config("/work")?;
    Ok(())
}

#[test]
fn a_path_with_a_nul_byte_is_refused_in_every_list() -> TestResult {
    for (deny_read, allow_write, deny_write) in [
        (r#"["/w/x\u0000--cap-add\u0000ALL"]"#, "[]", "[]"),
        ("[]", r#"["/w/x\u0000y"]"#, "[]"),
        ("[]", r#"["/w"]"#, r#"["/w/x\u0000--cap-add\u0000ALL"]"#),
    ] {
        let dir = tempfile::tempdir()?;
        let json = with_fs(deny_read, allow_write, deny_write);
        let refused = SandboxSettings::from_json(&json)
            .map_err(|error| error.to_string())
            .and_then(|settings| {
                settings
                    .into_config(dir.path())
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });
        let message = refused.err().ok_or("a NUL byte reaches the launch")?;
        assert!(message.contains("NUL"), "{message}");
    }
    Ok(())
}

#[test]
fn a_per_entry_inject_host_that_is_too_broad_is_refused() -> TestResult {
    let json = with_credentials(
        r#"["api.example.com"]"#,
        true,
        r#"{"envVars": [{"name": "GH_TOKEN", "mode": "mask", "injectHosts": ["*.com"]}]}"#,
    );
    SandboxSettings::from_json(&json)
        .and_then(|settings| settings.into_config("/work"))
        .err()
        .ok_or("a top-level wildcard is too broad")?;
    Ok(())
}

#[test]
fn the_removed_block_level_inject_hosts_key_is_refused() -> TestResult {
    let err = refused(&with_credentials(
        r#"["api.example.com"]"#,
        true,
        r#"{"injectHosts": ["api.example.com"], "envVars": [{"name": "T", "mode": "mask"}]}"#,
    ))?;
    assert!(err.message().contains("injectHosts"), "{err}");
    Ok(())
}

#[test]
fn keys_that_only_a_masked_entry_uses_are_ignored_on_a_deny_entry() -> TestResult {
    for credentials in [
        r#"{"envVars": [{"name": "GH_TOKEN", "mode": "deny", "injectHosts": ["api.github.com"]}]}"#,
        r#"{"envVars": [{"name": "DATABASE_URL", "mode": "deny", "extract": ":([^@]+)@"}]}"#,
        r#"{"envVars": [{"name": "CI_JOB_JWT", "mode": "deny", "decode": "jwt"}]}"#,
        r#"{"files": [{"path": "/home/me/.netrc", "mode": "deny", "extract": "password (\\S+)"}]}"#,
        r#"{"files": [{"path": "/home/me/.config/app/credentials", "mode": "deny", "decode": "jwt"}]}"#,
    ] {
        let config = credential_config(r#"["api.github.com"]"#, false, credentials)?;
        assert_eq!(
            config.bindings()[0].mode(),
            trogon_sandbox_contract::SecretMode::Deny,
            "{credentials}"
        );
    }
    Ok(())
}

#[test]
fn an_empty_ca_cert_path_is_refused() -> TestResult {
    refused(&with_network_extra(
        "tlsTerminate",
        r#"{"caCertPath": "", "caKeyPath": "/etc/ca.key"}"#,
    ))?;
    refused(&with_network_extra(
        "tlsTerminate",
        r#"{"caCertPath": "/etc/ca.crt", "caKeyPath": ""}"#,
    ))?;
    Ok(())
}

#[test]
fn an_empty_extra_ca_cert_path_is_refused() -> TestResult {
    refused(&with_network_extra("tlsTerminate", r#"{"extraCaCertPaths": [""]}"#))?;
    accepted(&with_network_extra(
        "tlsTerminate",
        r#"{"extraCaCertPaths": ["/etc/extra.pem"]}"#,
    ))?;
    Ok(())
}

#[test]
fn the_windows_block_ignores_keys_that_only_the_profile_read_used() -> TestResult {
    accepted(&with_extra(
        "windows",
        r#"{"profileRead": true, "profileReadExclusions": ["C:\\Users\\me\\.ssh"], "profileReadAllow": []}"#,
    ))?;
    Ok(())
}

#[test]
fn each_list_takes_a_path_marked_literal_beside_a_string() -> TestResult {
    let policy = accepted(&document(
        r#"{"allowedDomains": [], "deniedDomains": []}"#,
        r#"{"denyRead": ["/a/*.env", {"path": "/b/[WIP] project", "literal": true}],
            "allowRead": [{"path": "/c/build*", "literal": true}, "/c/keep"],
            "allowWrite": [{"path": "/d/notes (draft?)", "literal": true}, "/d/out"],
            "denyWrite": [{"path": "/e/open[bracket", "literal": true}, "/e/*.key"]}"#,
    ))?
    .policy()?
    .fs
    .ok_or("no filesystem policy")?;
    let kinds = |entries: &[FsPathEntry]| -> Vec<bool> {
        entries
            .iter()
            .map(|entry| matches!(entry, FsPathEntry::Glob { .. }))
            .collect()
    };
    assert_eq!(kinds(policy.deny_read()), [true, false]);
    assert_eq!(kinds(policy.allow_read_within_deny()), [false, false]);
    assert_eq!(kinds(policy.allow_write()), [false, false]);
    assert_eq!(kinds(policy.deny_write()), [false, true]);
    Ok(())
}

#[test]
fn an_allow_with_a_bracket_that_spells_no_set_is_kept_as_written() -> TestResult {
    let policy = accepted(&document(
        r#"{"allowedDomains": [], "deniedDomains": []}"#,
        r#"{"denyRead": ["/a/open[bracket"],
            "allowRead": ["/c/keep]"],
            "allowWrite": ["/d/out[1", "/d/plain"],
            "denyWrite": ["/e/open[bracket"]}"#,
    ))?
    .policy()?
    .fs
    .ok_or("no filesystem policy")?;
    let kinds = |entries: &[FsPathEntry]| -> Vec<bool> {
        entries
            .iter()
            .map(|entry| matches!(entry, FsPathEntry::Glob { .. }))
            .collect()
    };
    assert_eq!(kinds(policy.deny_read()), [false]);
    assert_eq!(kinds(policy.allow_read_within_deny()), [false]);
    assert_eq!(kinds(policy.allow_write()), [false, false]);
    assert_eq!(kinds(policy.deny_write()), [false]);
    Ok(())
}

#[test]
fn the_tunnel_cap_and_handshake_timeout_accept_their_bounds() -> TestResult {
    for tls in [
        r#"{"maxTunnels": 1}"#,
        r#"{"maxTunnels": 65536}"#,
        r#"{"handshakeTimeoutMs": 100}"#,
        r#"{"handshakeTimeoutMs": 600000}"#,
    ] {
        accepted(&with_network_extra("tlsTerminate", tls))?;
    }
    Ok(())
}

#[test]
fn a_tunnel_cap_and_handshake_timeout_of_the_wrong_type_are_refused() -> TestResult {
    for tls in [
        r#"{"maxTunnels": "12"}"#,
        r#"{"maxTunnels": null}"#,
        r#"{"handshakeTimeoutMs": null}"#,
    ] {
        refused(&with_network_extra("tlsTerminate", tls))?;
    }
    Ok(())
}

#[test]
fn allow_local_binding_takes_a_boolean_and_nothing_else() -> TestResult {
    accepted(&with_network_extra("allowLocalBinding", "true"))?;
    accepted(&with_network_extra("allowLocalBinding", "false"))?;
    for value in [r#""all""#, "1", "[3000, 3099]", "null"] {
        refused(&with_network_extra("allowLocalBinding", value))?;
    }
    Ok(())
}

#[test]
fn only_the_three_ambiguous_request_target_names_are_accepted() -> TestResult {
    for value in [r#""""#, r#""asspelled""#, "true", "null", "1"] {
        refused(&with_network_extra("ambiguousRequestTarget", value))?;
    }
    Ok(())
}

fn reported(json: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(refused(json)?.to_string())
}

fn credential_files(entry: &str) -> String {
    with_extra("credentials", &format!(r#"{{"files": [{entry}]}}"#))
}

// Integer options with a lower and an upper bound.
#[test]
fn a_number_outside_its_bounds_names_the_bound() -> TestResult {
    let port = |value: &str| reported(&with_network_extra("httpProxyPort", value));
    assert_eq!(
        port("0")?,
        "network.httpProxyPort: Number must be greater than or equal to 1"
    );
    assert_eq!(
        port("-1")?,
        "network.httpProxyPort: Number must be greater than or equal to 1"
    );
    assert_eq!(
        port("70000")?,
        "network.httpProxyPort: Number must be less than or equal to 65535"
    );
    assert_eq!(
        reported(&with_network_extra("socksProxyPort", "0"))?,
        "network.socksProxyPort: Number must be greater than or equal to 1"
    );
    let tls = |option: &str| reported(&with_network_extra("tlsTerminate", option));
    assert_eq!(
        tls(r#"{"maxTunnels": 0}"#)?,
        "network.tlsTerminate.maxTunnels: Number must be greater than or equal to 1"
    );
    assert_eq!(
        tls(r#"{"maxTunnels": 65537}"#)?,
        "network.tlsTerminate.maxTunnels: Number must be less than or equal to 65536"
    );
    assert_eq!(
        tls(r#"{"handshakeTimeoutMs": 99}"#)?,
        "network.tlsTerminate.handshakeTimeoutMs: Number must be greater than or equal to 100"
    );
    assert_eq!(
        tls(r#"{"handshakeTimeoutMs": 600001}"#)?,
        "network.tlsTerminate.handshakeTimeoutMs: Number must be less than or equal to 600000"
    );
    assert_eq!(
        reported(&with_extra("mandatoryDenySearchDepth", "0"))?,
        "mandatoryDenySearchDepth: Number must be greater than or equal to 1"
    );
    assert_eq!(
        reported(&with_extra("mandatoryDenySearchDepth", "11"))?,
        "mandatoryDenySearchDepth: Number must be less than or equal to 10"
    );
    Ok(())
}

#[test]
fn a_fractional_number_is_refused_as_a_float_and_again_when_out_of_bounds() -> TestResult {
    let port = |value: &str| reported(&with_network_extra("httpProxyPort", value));
    assert_eq!(port("1.5")?, "network.httpProxyPort: Expected integer, received float");
    assert_eq!(
        port("0.5")?,
        "network.httpProxyPort: Expected integer, received float; \
         network.httpProxyPort: Number must be greater than or equal to 1"
    );
    assert_eq!(
        port("70000.5")?,
        "network.httpProxyPort: Expected integer, received float; \
         network.httpProxyPort: Number must be less than or equal to 65535"
    );
    accepted(&with_network_extra("httpProxyPort", "3.0"))?;
    Ok(())
}

#[test]
fn a_value_that_is_no_number_is_refused_by_its_kind() -> TestResult {
    let port = |value: &str| reported(&with_network_extra("httpProxyPort", value));
    assert_eq!(
        port(r#""8""#)?,
        "network.httpProxyPort: Expected number, received string"
    );
    assert_eq!(
        port("true")?,
        "network.httpProxyPort: Expected number, received boolean"
    );
    assert_eq!(port("[]")?, "network.httpProxyPort: Expected number, received array");
    assert_eq!(port("{}")?, "network.httpProxyPort: Expected number, received object");
    assert_eq!(
        reported(&with_extra("mandatoryDenySearchDepth", "null"))?,
        "mandatoryDenySearchDepth: Expected number, received null"
    );
    Ok(())
}

// A pair of two bounded integers that must also be ordered.
#[test]
fn the_windows_port_range_names_each_failing_position() -> TestResult {
    let range = |value: &str| reported(&with_extra("windows", &format!(r#"{{"proxyPortRange": {value}}}"#)));
    assert_eq!(
        range("[0, 5]")?,
        "windows.proxyPortRange.0: Number must be greater than or equal to 1"
    );
    assert_eq!(
        range("[1, 70000]")?,
        "windows.proxyPortRange.1: Number must be less than or equal to 65535; \
         windows.proxyPortRange: low must be \u{2264} high and range width \u{2264} 64"
    );
    assert_eq!(
        range("[10, 5]")?,
        "windows.proxyPortRange: low must be \u{2264} high and range width \u{2264} 64"
    );
    assert_eq!(
        range("[1, 2.5]")?,
        "windows.proxyPortRange.1: Expected integer, received float"
    );
    assert_eq!(
        range("[0.5, 70000.5]")?,
        "windows.proxyPortRange.0: Expected integer, received float; \
         windows.proxyPortRange.0: Number must be greater than or equal to 1; \
         windows.proxyPortRange.1: Expected integer, received float; \
         windows.proxyPortRange.1: Number must be less than or equal to 65535; \
         windows.proxyPortRange: low must be \u{2264} high and range width \u{2264} 64"
    );
    assert_eq!(
        range("[1]")?,
        "windows.proxyPortRange: Array must contain at least 2 element(s)"
    );
    assert_eq!(
        range("[1, 2, 3]")?,
        "windows.proxyPortRange: Array must contain at most 2 element(s)"
    );
    assert_eq!(
        range(r#""x""#)?,
        "windows.proxyPortRange: Expected array, received string"
    );
    assert_eq!(
        range(r#"["a", null]"#)?,
        "windows.proxyPortRange.0: Expected number, received string; \
         windows.proxyPortRange.1: Expected number, received null"
    );
    accepted(&with_extra("windows", r#"{"proxyPortRange": [1, 65]}"#))?;
    Ok(())
}

// Options that accept one of a fixed set of strings.
#[test]
fn an_unknown_enum_value_lists_the_accepted_values() -> TestResult {
    assert_eq!(
        reported(&credential_files(r#"{"path": "/x", "mode": "x"}"#))?,
        "credentials.files.0.mode: Invalid enum value. Expected 'deny' | 'mask', received 'x'"
    );
    assert_eq!(
        reported(&credential_files(
            r#"{"path": "/x", "mode": "deny", "onExtractNoMatch": "x"}"#
        ))?,
        "credentials.files.0.onExtractNoMatch: Invalid enum value. \
         Expected 'warn' | 'deny' | 'error', received 'x'"
    );
    assert_eq!(
        reported(&with_extra(
            "credentials",
            r#"{"envVars": [{"name": "A", "mode": "deny", "decode": "x"}]}"#
        ))?,
        "credentials.envVars.0.decode: Invalid enum value. Expected 'jwt', received 'x'"
    );
    assert_eq!(
        reported(&with_network_extra("ambiguousRequestTarget", r#""x""#))?,
        "network.ambiguousRequestTarget: Invalid enum value. \
         Expected 'refuse' | 'asSpelled' | 'normalize', received 'x'"
    );
    Ok(())
}

#[test]
fn an_enum_given_another_kind_of_value_is_refused_by_its_kind() -> TestResult {
    assert_eq!(
        reported(&credential_files(r#"{"path": "/x", "mode": 3}"#))?,
        "credentials.files.0.mode: Expected 'deny' | 'mask', received number"
    );
    assert_eq!(
        reported(&with_network_extra("ambiguousRequestTarget", "null"))?,
        "network.ambiguousRequestTarget: Expected 'refuse' | 'asSpelled' | 'normalize', received null"
    );
    assert_eq!(
        reported(&with_extra(
            "credentials",
            r#"{"sigv4": {"streaming": "x", "presigned": 4, "sigv4a": true}}"#
        ))?,
        "credentials.sigv4.streaming: Invalid enum value. \
         Expected 'deny' | 'passthrough', received 'x'; \
         credentials.sigv4.presigned: Expected 'deny' | 'passthrough', received number; \
         credentials.sigv4.sigv4a: Expected 'deny' | 'passthrough', received boolean"
    );
    Ok(())
}

// A filesystem entry is either a path string or an object with a path and the literal flag.
#[test]
fn a_filesystem_entry_that_is_no_path_says_what_is_expected() -> TestResult {
    let entry = r#"Expected a path, or { "path": "<path>", "literal": true }"#;
    for value in [
        r#"{"path": "/x", "literal": false}"#,
        r#"{"path": "/x", "literal": "a"}"#,
        r#"{"path": "/x"}"#,
        r#"{"literal": true}"#,
        r#"{"path": 5, "literal": true}"#,
        "5",
        "null",
        "true",
        "[]",
    ] {
        assert_eq!(
            reported(&with_fs(&format!("[{value}]"), "[]", "[]"))?,
            format!("filesystem.denyRead.0: {entry}"),
            "{value}"
        );
    }
    assert_eq!(
        reported(&with_fs(r#"[{"path": "", "literal": true}]"#, "[]", "[]"))?,
        "filesystem.denyRead.0.path: Path cannot be empty"
    );
    assert_eq!(
        reported(&with_fs(r#"[""]"#, "[]", "[]"))?,
        "filesystem.denyRead.0: Path cannot be empty"
    );
    Ok(())
}

// Minimum lengths, URLs, UUIDs and patterns each name what they require.
#[test]
fn empty_strings_arrays_urls_uuids_and_names_each_name_what_they_require() -> TestResult {
    let empty = "String must contain at least 1 character(s)";
    assert_eq!(
        reported(&with_network_extra(
            "mitmProxy",
            r#"{"socketPath": "", "domains": ["a.com"]}"#
        ))?,
        format!("network.mitmProxy.socketPath: {empty}")
    );
    assert_eq!(
        reported(&with_network_extra(
            "mitmProxy",
            r#"{"socketPath": "/s", "domains": []}"#
        ))?,
        "network.mitmProxy.domains: Array must contain at least 1 element(s)"
    );
    assert_eq!(
        reported(&with_network_extra("tlsTerminate", r#"{"caCertPath": ""}"#))?,
        format!("network.tlsTerminate.caCertPath: {empty}")
    );
    assert_eq!(
        reported(&with_network_extra("deniedDomainReasons", r#"{"a.com": ""}"#))?,
        format!("network.deniedDomainReasons.a.com: {empty}")
    );
    assert_eq!(
        reported(&with_extra("windows", r#"{"sandboxUser": ""}"#))?,
        format!("windows.sandboxUser: {empty}")
    );
    assert_eq!(
        reported(&with_extra("windows", r#"{"sandboxUser": "aaaaaaaaaaaaaaaaaaaaa"}"#))?,
        "windows.sandboxUser: Windows local usernames are limited to 20 characters"
    );
    assert_eq!(
        reported(&with_extra("windows", r#"{"fenceHelper": {"path": ""}}"#))?,
        format!("windows.fenceHelper.path: {empty}")
    );
    assert_eq!(
        reported(&with_extra("windows", r#"{"wfpSublayerGuid": "x"}"#))?,
        "windows.wfpSublayerGuid: Invalid uuid"
    );
    assert_eq!(
        reported(&with_network_extra("parentProxy", r#"{"http": "nope"}"#))?,
        "network.parentProxy.http: Invalid url"
    );
    assert_eq!(
        reported(&credential_files(r#"{"path": "", "mode": "deny"}"#))?,
        "credentials.files.0.path: Path cannot be empty"
    );
    assert_eq!(
        reported(&credential_files(
            r#"{"path": "/x", "mode": "deny", "maskClaims": [""]}"#
        ))?,
        format!(
            "credentials.files.0.maskClaims.0: {empty}; credentials.files.0.maskClaims: maskClaims requires decode - it names claims inside the decoded payload. Set decode (e.g. \"jwt\"), or remove maskClaims to mask the extracted value whole."
        )
    );
    assert_eq!(
        reported(&with_extra(
            "credentials",
            r#"{"envVars": [{"name": "1a", "mode": "deny"}]}"#
        ))?,
        "credentials.envVars.0.name: Environment variable name must start with a letter or \
         underscore and contain only letters, digits, and underscores"
    );
    Ok(())
}

// A binary path must be non-empty and absolute; the empty string fails both checks.
#[test]
fn a_binary_path_is_worded_by_each_check_it_fails() -> TestResult {
    assert_eq!(
        reported(&with_extra("bwrapPath", r#""""#))?,
        "bwrapPath: Path cannot be empty; bwrapPath: Binary path must be absolute"
    );
    assert_eq!(
        reported(&with_extra("bwrapPath", r#""rel""#))?,
        "bwrapPath: Binary path must be absolute"
    );
    Ok(())
}

// A plain domain pattern takes no port suffix.
#[test]
fn a_domain_list_without_ports_words_a_refused_entry_without_the_port_hint() -> TestResult {
    let message = constants::DOMAIN_PATTERN;
    assert!(!message.contains("port"), "{message}");
    assert_eq!(
        reported(&with_network_extra(
            "mitmProxy",
            r#"{"socketPath": "/s", "domains": ["a.com:443"]}"#
        ))?,
        format!("network.mitmProxy.domains.0: {message}")
    );
    assert_eq!(
        reported(&with_network_extra("tlsTerminate", r#"{"excludeDomains": [""]}"#))?,
        format!("network.tlsTerminate.excludeDomains.0: {message}")
    );
    assert_eq!(
        reported(&credential_files(
            r#"{"path": "/x", "mode": "deny", "injectHosts": [""]}"#
        ))?,
        format!(
            "credentials.files.0.injectHosts.0: {message}; credentials.files.0.injectHosts.0: injectHosts entry \"\" is not reachable via network.allowedDomains - add \"\" (or a covering wildcard) to allowedDomains, or remove it from injectHosts."
        )
    );
    Ok(())
}

// Claim names only need to be non-empty.
#[test]
fn a_claim_name_has_no_upper_length_limit() -> TestResult {
    let long = "c".repeat(300);
    accepted(&with_extra(
        "credentials",
        &format!(r#"{{"files": [{{"path": "/x", "mode": "deny", "decode": "jwt", "maskClaims": ["{long}"]}}]}}"#),
    ))?;
    Ok(())
}

// An environment variable name is checked with a pattern only.
#[test]
fn an_env_var_name_has_no_upper_length_limit() -> TestResult {
    let long = "A".repeat(300);
    accepted(&with_extra(
        "credentials",
        &format!(r#"{{"envVars": [{{"name": "{long}", "mode": "deny"}}]}}"#),
    ))?;
    Ok(())
}

// A plain domain list entry has no bracketed IPv6 form;
// allowedDomains alone accepts one.
#[test]
fn a_bracketed_ipv6_literal_is_refused_in_plain_domain_lists() -> TestResult {
    let message = constants::DOMAIN_PATTERN;
    for literal in ["[::1]", "[2001:db8::1]"] {
        assert_eq!(
            reported(&with_network_extra(
                "tlsTerminate",
                &format!(r#"{{"excludeDomains": ["{literal}"]}}"#)
            ))?,
            format!("network.tlsTerminate.excludeDomains.0: {message}")
        );
        assert_eq!(
            reported(&with_network_extra(
                "mitmProxy",
                &format!(r#"{{"socketPath": "/s", "domains": ["{literal}"]}}"#)
            ))?,
            format!("network.mitmProxy.domains.0: {message}")
        );
    }
    accepted(&with_domains(r#"["[::1]"]"#, "[]"))?;
    Ok(())
}

// Every credentials.sigv4 field may be omitted but never null.
#[test]
fn a_null_sigv4_section_or_choice_is_refused() -> TestResult {
    assert_eq!(
        reported(&with_extra("credentials", r#"{"sigv4": null}"#))?,
        "credentials.sigv4: Expected object, received null"
    );
    assert_eq!(
        reported(&with_extra("credentials", r#"{"sigv4": {"sigv4a": null}}"#))?,
        "credentials.sigv4.sigv4a: Expected 'deny' | 'passthrough', received null"
    );
    Ok(())
}

// Every object-typed section refuses an array.
#[test]
fn an_array_in_place_of_a_section_is_refused() -> TestResult {
    let expected = |path: &str| format!("{path}: Expected object, received array");
    for (key, path) in [
        ("windows", "windows"),
        ("git", "git"),
        ("ripgrep", "ripgrep"),
        ("seccomp", "seccomp"),
        ("credentials", "credentials"),
    ] {
        assert_eq!(reported(&with_extra(key, "[]"))?, expected(path), "{key}");
    }
    for key in ["tlsTerminate", "mitmProxy", "parentProxy"] {
        assert_eq!(
            reported(&with_network_extra(key, "[]"))?,
            expected(&format!("network.{key}")),
            "{key}"
        );
    }
    assert_eq!(
        reported(&with_extra("credentials", r#"{"sigv4": []}"#))?,
        expected("credentials.sigv4")
    );
    assert_eq!(
        reported(&with_extra("windows", r#"{"fenceHelper": []}"#))?,
        expected("windows.fenceHelper")
    );
    for key in ["envVars", "files", "awsPairs"] {
        assert_eq!(
            reported(&with_extra("credentials", &format!(r#"{{"{key}": [[]]}}"#)))?,
            expected(&format!("credentials.{key}.0")),
            "{key}"
        );
    }
    Ok(())
}

// Denied resolved addresses are IPv4 or IPv6 literals or CIDR ranges.
#[test]
fn denied_resolved_addresses_are_read_as_address_ranges() -> TestResult {
    let message = "Invalid IP address or CIDR range. Use an IPv4/IPv6 literal or CIDR, e.g. \"10.0.0.0/8\", \"192.168.1.10\", \"fc00::/7\" (IPv6 unbracketed).";
    for entry in [
        "10.0.0.0/33",
        "::1/129",
        "1.2.3.4 ",
        "1.2.3.4/",
        "01.2.3.4",
        "1.2.3",
        "127.1",
        "10.0.0.0/8/8",
        "",
    ] {
        assert_eq!(
            reported(&with_network_extra(
                "deniedResolvedAddresses",
                &serde_json::json!([entry]).to_string()
            ))?,
            format!("network.deniedResolvedAddresses.0: {message}"),
            "{entry:?}"
        );
    }
    for entry in [
        "1.2.3.4/0",
        "fe80::1%eth0",
        "::ffff:1.2.3.4",
        "::ffff:1.2.3.4/96",
        "1.2.3.4/08",
        "FC00::/7",
    ] {
        accepted(&with_network_extra(
            "deniedResolvedAddresses",
            &serde_json::json!([entry]).to_string(),
        ))?;
    }
    assert_eq!(
        reported(&with_network_extra(
            "deniedResolvedAddresses",
            r#"["::ffff:1.2.3.4/95"]"#
        ))?,
        format!("network.deniedResolvedAddresses.0: {message}")
    );
    Ok(())
}

// Only the wildcard position of a Mach service name is checked.
#[test]
fn mach_service_names_are_refused_only_for_a_misplaced_wildcard() -> TestResult {
    let message =
        "Wildcards are only allowed as a single trailing \"*\" (e.g., \"com.example.*\" or \"*\" for all services).";
    assert_eq!(
        reported(&with_network_extra("allowMachLookup", r#"["**"]"#))?,
        format!("network.allowMachLookup.0: {message}")
    );
    for entry in ["*", "a*", "", "a\u{1}b"] {
        accepted(&with_network_extra(
            "allowMachLookup",
            &serde_json::json!([entry]).to_string(),
        ))?;
    }
    Ok(())
}

// An extract pattern must be a valid regular expression.
#[test]
fn an_extract_pattern_that_does_not_compile_is_refused() -> TestResult {
    let extract = |pattern: &str| {
        reported(&refined_credentials(
            "[]",
            true,
            "",
            &format!(
                r#"{{"files": [{{"path": "/x", "mode": "mask", "extract": {}}}]}}"#,
                serde_json::json!(pattern)
            ),
        ))
    };
    let invalid = |source: &str, reason: &str| {
        format!(
            "credentials.files.0.extract: extract is not a valid regular expression: Invalid regular expression: /{source}/: {reason}"
        )
    };
    assert_eq!(extract("(")?, invalid("(", "Unterminated group"));
    assert_eq!(extract("(a")?, invalid("(a", "Unterminated group"));
    assert_eq!(extract("a)")?, invalid("a)", "Unmatched ')'"));
    assert_eq!(extract("*a")?, invalid("*a", "Nothing to repeat"));
    assert_eq!(extract("a**(b)")?, invalid("a**(b)", "Nothing to repeat"));
    assert_eq!(extract("[a")?, invalid("[a", "Unterminated character class"));
    assert_eq!(
        extract("a{2,1}")?,
        invalid("a{2,1}", "numbers out of order in {} quantifier")
    );
    assert_eq!(
        extract("[z-a](b)")?,
        invalid("[z-a](b)", "Range out of order in character class")
    );
    assert_eq!(extract("\\")?, invalid("\\", "\\ at end of pattern"));
    assert_eq!(extract("(?i)a(b)")?, invalid("(?i)a(b)", "Invalid group"));
    assert_eq!(extract("(?P<n>a)")?, invalid("(?P<n>a)", "Invalid group"));
    assert_eq!(extract("(?<1a>x)")?, invalid("(?<1a>x)", "Invalid capture group name"));
    assert_eq!(extract("a/b(")?, invalid("a\\/b(", "Unterminated group"));
    let no_group = "credentials.files.0.extract: extract must contain at least one capturing group - group 1 is the credential value to mask (e.g. \"token:\\s*(\\S+)\").";
    assert_eq!(extract("abc")?, no_group);
    assert_eq!(extract("(?:a)")?, no_group);
    Ok(())
}

fn refined_credentials(allowed: &str, tls: bool, network_extra: &str, credentials: &str) -> String {
    let tls = if tls { r#", "tlsTerminate": {}"# } else { "" };
    format!(
        r#"{{"network": {{"allowedDomains": {allowed}, "deniedDomains": []{tls}{network_extra}}},
            "filesystem": {{"denyRead": [], "allowWrite": [], "denyWrite": []}},
            "credentials": {credentials}}}"#
    )
}

// The cross-field checks run after the structural checks and are skipped when
// one of them fails to parse its input, so a wrong type or enum aborts it.
#[test]
fn the_cross_field_checks_run_after_the_structural_ones_and_yield_to_a_type_error() -> TestResult {
    let unreachable = |host: &str| {
        format!(
            "credentials.envVars.0.injectHosts.0: injectHosts entry \"{host}\" is not reachable via network.allowedDomains - add \"{host}\" (or a covering wildcard) to allowedDomains, or remove it from injectHosts."
        )
    };
    let env = r#"{"envVars": [{"name": "A", "mode": "mask", "injectHosts": ["zz.com"]}]}"#;

    assert_eq!(
        reported(&refined_credentials("[]", true, r#", "httpProxyPort": 70000"#, env))?,
        format!(
            "network.httpProxyPort: Number must be less than or equal to 65535; {}",
            unreachable("zz.com")
        )
    );
    assert_eq!(
        reported(&refined_credentials("[]", true, r#", "httpProxyPort": 1.5"#, env))?,
        format!(
            "network.httpProxyPort: Expected integer, received float; {}",
            unreachable("zz.com")
        )
    );
    assert_eq!(
        reported(&refined_credentials("[]", true, r#", "httpProxyPort": "x""#, env))?,
        "network.httpProxyPort: Expected number, received string"
    );
    assert_eq!(
        reported(&refined_credentials(
            "[]",
            true,
            "",
            r#"{"envVars": [{"name": "A", "mode": "bad", "injectHosts": ["zz.com"]}]}"#
        ))?,
        "credentials.envVars.0.mode: Invalid enum value. Expected 'deny' | 'mask', received 'bad'"
    );
    assert_eq!(
        reported(&refined_credentials(
            "[]",
            true,
            "",
            r#"{"envVars": [{"mode": "mask", "injectHosts": ["zz.com"]}]}"#
        ))?,
        "credentials.envVars.0.name: Required"
    );
    Ok(())
}

// Cross-field problems are reported in the order the checks run.
#[test]
fn every_cross_field_problem_is_reported_in_check_order() -> TestResult {
    let json = refined_credentials(
        "[]",
        false,
        "",
        r#"{"envVars": [{"name": "A", "mode": "mask", "injectHosts": ["zz.com"], "maskClaims": []}],
            "files": [{"path": "/x/", "mode": "mask", "injectHosts": []}],
            "awsPairs": [{"accessKeyIdVar": "Q", "secretAccessKeyVar": "R"}]}"#,
    )
    .replacen(r#""denyRead": []"#, r#""denyRead": ["/x/*/"]"#, 1);
    let text = reported(&json)?;
    let order = [
        "filesystem.denyRead.0: Deny glob \"/x/*/\" ends in a separator",
        "credentials.envVars.0.injectHosts.0: injectHosts entry \"zz.com\" is not reachable",
        "credentials.envVars.0.maskClaims: maskClaims requires decode",
        "credentials.envVars.0.maskClaims: maskClaims is explicitly empty",
        "credentials.files.0: injectHosts is explicitly empty",
        "credentials.files.0.path: ",
        "credentials.awsPairs.0.accessKeyIdVar: ",
        "credentials: Credential masking requires network.tlsTerminate",
    ];
    let mut from = 0;
    for part in order {
        let at = text[from..]
            .find(part)
            .ok_or_else(|| format!("{part:?} missing or out of order in {text}"))?;
        from += at + part.len();
    }
    Ok(())
}

// A document with several refused options renders every one, joined by "; ".
#[test]
fn several_serde_issues_are_all_reported_in_one_message() -> TestResult {
    let json = document(
        r#"{"allowedDomains": ["a.com:x"], "deniedDomains": ["*.com"]}"#,
        r#"{"denyRead": [], "allowWrite": [5], "denyWrite": []}"#,
    );
    let text = reported(&json)?;
    assert!(text.contains("network.allowedDomains.0: "), "{text}");
    assert!(text.contains("network.deniedDomains.0: "), "{text}");
    assert!(text.contains("filesystem.allowWrite.0: "), "{text}");
    assert_eq!(text.matches("; ").count(), 2, "{text}");
    Ok(())
}

mod host_match_tests;
