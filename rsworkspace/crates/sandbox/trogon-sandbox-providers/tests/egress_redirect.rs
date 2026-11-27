use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_providers::{
    EGRESS_PROXY_EXEMPT_UID_ENV, EGRESS_PROXY_LEASE_ID_ENV, EGRESS_PROXY_VIOLATION_NATS_URL_ENV, EgressViolationEvents,
    SinkHost, UID_CHANGE_CAPABILITIES, ViolationSinkEndpoint, ViolationSinkEndpointError, egress_redirect_command,
    exempt_uid_env_pair, reject_proxy_uid_workload, rewrite_loopback_host,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn redirect_command_exempts_only_the_proxy_uid_ahead_of_the_redirect() {
    assert_eq!(
        egress_redirect_command(3128),
        "iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner 65531 -j RETURN && iptables -t nat -A OUTPUT -p tcp ! -d 127.0.0.1 -j REDIRECT --to-port 3128"
    );
}

#[test]
fn redirect_command_no_longer_depends_on_socket_marks() {
    assert!(!egress_redirect_command(3128).contains("mark"));
}

#[test]
fn the_proxy_is_told_which_uid_the_redirect_exempts() {
    assert_eq!(
        exempt_uid_env_pair(),
        (EGRESS_PROXY_EXEMPT_UID_ENV, "65531".to_string())
    );
    assert_eq!(EGRESS_PROXY_EXEMPT_UID_ENV, "EGRESS_PROXY_EXEMPT_UID");
}

#[test]
fn a_workload_user_that_is_the_proxy_uid_is_rejected() {
    for user in ["65531", "65531:65531", "65531:0"] {
        assert!(reject_proxy_uid_workload(user).is_err(), "{user:?}");
    }
}

#[test]
fn the_rejection_names_the_user_and_the_proxy_uid() {
    let message = reject_proxy_uid_workload("65531:0")
        .err()
        .map(|error| error.to_string());
    assert_eq!(
        message.as_deref(),
        Some("user: \"65531:0\" is the egress proxy uid 65531; the workload must run as a different uid")
    );
}

#[test]
fn other_workload_users_are_accepted() {
    for user in ["0", "65532", "1000:1000", "nonroot", "nonroot:65531", ""] {
        assert!(reject_proxy_uid_workload(user).is_ok(), "{user:?}");
    }
}

#[test]
fn uid_change_capabilities_cover_setuid_and_setgid() {
    assert_eq!(UID_CHANGE_CAPABILITIES, ["SETUID", "SETGID"]);
}

#[test]
fn rewrite_loopback_host_replaces_only_loopback_hosts() {
    let to = "host.docker.internal";
    let cases = [
        ("nats://127.0.0.1:4222", "nats://host.docker.internal:4222"),
        (
            "nats://user:pw@localhost:4222",
            "nats://user:pw@host.docker.internal:4222",
        ),
        ("nats://[::1]:4222", "nats://host.docker.internal:4222"),
        ("nats://localhost", "nats://host.docker.internal"),
        (
            "nats://localhost:4222/subject",
            "nats://host.docker.internal:4222/subject",
        ),
        ("nats://nats.internal:4222", "nats://nats.internal:4222"),
        ("nats://[::1", "nats://[::1"),
        ("not a url", "not a url"),
    ];
    for (url, expected) in cases {
        assert_eq!(rewrite_loopback_host(url, to), expected, "{url}");
    }
}

#[test]
fn endpoint_defaults_to_nats_port() -> TestResult {
    let endpoints = ViolationSinkEndpoint::parse_all("nats://user:pw@nats.svc")?;
    assert_eq!(
        endpoints,
        vec![ViolationSinkEndpoint {
            host: SinkHost::Name("nats.svc".to_string()),
            port: 4222
        }]
    );
    Ok(())
}

#[test]
fn endpoints_parse_ipv4_literals_ports_and_lists() -> TestResult {
    let endpoints = ViolationSinkEndpoint::parse_all("nats://10.0.0.7:4223/path, tls://dns-name_1.example:5000")?;
    assert_eq!(
        endpoints,
        vec![
            ViolationSinkEndpoint {
                host: SinkHost::Ipv4("10.0.0.7".parse()?),
                port: 4223
            },
            ViolationSinkEndpoint {
                host: SinkHost::Name("dns-name_1.example".to_string()),
                port: 5000
            },
        ]
    );
    Ok(())
}

#[test]
fn unparseable_urls_produce_no_events() {
    for url in [
        "",
        "nats://",
        "nats://host:notaport",
        "nats://host:0",
        "nats://[::1]:4222",
        "nats://a;reboot:4222",
        "nats://ok:4222,nats://",
    ] {
        assert_eq!(
            ViolationSinkEndpoint::parse_all(url),
            Err(ViolationSinkEndpointError(url.to_string())),
            "{url}"
        );
        assert!(
            EgressViolationEvents::try_new(url, LeaseId::new("l")).is_none(),
            "{url}"
        );
    }
}

#[test]
fn the_endpoint_error_message_quotes_the_url() {
    assert_eq!(
        ViolationSinkEndpointError("bad".to_string()).to_string(),
        "unusable NATS url for egress violation events: \"bad\""
    );
}

#[test]
fn events_hand_the_proxy_its_url_and_lease() -> TestResult {
    let events = EgressViolationEvents::new("nats://nats.svc:4222", LeaseId::new("lease-1"))?;
    assert_eq!(
        events.env_pairs(),
        [
            (EGRESS_PROXY_VIOLATION_NATS_URL_ENV, "nats://nats.svc:4222"),
            (EGRESS_PROXY_LEASE_ID_ENV, "lease-1"),
        ]
    );
    assert_eq!(
        EgressViolationEvents::try_new("nats://nats.svc:4222", LeaseId::new("lease-1")),
        Some(events)
    );
    Ok(())
}
